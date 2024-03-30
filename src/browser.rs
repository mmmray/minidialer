use std::future::Future;

use async_channel::{bounded, Receiver, Sender};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Html,
    routing::{any, get},
    Router,
};

use crate::BrowserCli;

type Pipe = (Sender<Message>, Receiver<Message>);

pub async fn main(args: BrowserCli) {
    let (browser_listen_queue_in, browser_listen_queue_out) = bounded(4096);

    let state = AppState {
        browser_listen_queue_in,
        browser_listen_queue_out,
        upstream: args.upstream.clone(),
    };

    let app = Router::new()
        .route("/", get(root))
        .route(
            "/dialer.js",
            get(|| async {
                (
                    [("Content-Type", "application/javascript")],
                    include_str!("../static/dialer.js"),
                )
            }),
        )
        .route(
            "/browser",
            any(|state, ws: WebSocketUpgrade| async {
                ws.on_upgrade(|ws| browser_handler(state, ws))
            }),
        )
        .route(
            "/client",
            any(|state, ws: WebSocketUpgrade| async {
                ws.on_upgrade(|ws| client_handler(state, ws))
            }),
        )
        .with_state(state);

    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[derive(Clone)]
struct AppState {
    upstream: String,
    browser_listen_queue_in: Sender<Pipe>,
    browser_listen_queue_out: Receiver<Pipe>,
}

async fn root() -> Html<&'static str> {
    Html(
        r#"
<!DOCTYPE html>
<script src=/dialer.js></script>
"#,
    )
}

async fn browser_handler(State(state): State<AppState>, socket: WebSocket) {
    let get_pipe = || async {
        let (sender2, receiver) = bounded(4096);
        let (sender, receiver2) = bounded(4096);

        let pipe = (sender2, receiver2);
        state.browser_listen_queue_in.send(pipe).await.unwrap();
        tracing::debug!(
            "added browser, now idle: {}",
            state.browser_listen_queue_out.len()
        );
        (sender, receiver)
    };

    mirror_websocket(get_pipe, socket, "browser_handler").await;
}

async fn client_handler(State(state): State<AppState>, socket: WebSocket) {
    let get_pipe = || async {
        loop {
            let (sender, receiver) = state.browser_listen_queue_out.recv().await.unwrap();
            tracing::debug!(
                "used browser, now idle: {}",
                state.browser_listen_queue_out.len()
            );

            let Ok(_) = sender.send(Message::Text(state.upstream.clone())).await else {
                tracing::debug!("channel broke while trying to dial, dropping");
                continue;
            };

            if let Ok(msg) = receiver.recv().await {
                if msg.into_data() == b"ready" {
                    break (sender, receiver);
                }
            }

            tracing::warn!(
                "the browser is not responding to dialer requests. check browser console?"
            );
        }
    };

    mirror_websocket(get_pipe, socket, "client_handler").await;
}

async fn mirror_websocket<F, P>(get_pipe: F, mut socket: WebSocket, log_tag: &'static str)
where
    F: Fn() -> P,
    P: Future<Output = Pipe>,
{
    let mut transmitted_anything = false;
    let mut from_network: Option<Message> = None;
    let mut from_channel: Option<Message> = None;

    loop {
        if transmitted_anything {
            tracing::debug!("dropping websocket connection because we already transmitted bytes");
            return;
        }

        let (sender, receiver) = get_pipe().await;

        loop {
            if let Some(ref msg) = from_network {
                if sender.send(msg.clone()).await.is_err() {
                    tracing::debug!(
                        "failed to forward network packet in {}, getting new channel",
                        log_tag
                    );
                    break;
                }

                from_network = None;
                transmitted_anything = true;
            } else if let Some(ref msg) = from_channel {
                if socket.send(msg.clone()).await.is_err() {
                    tracing::debug!(
                        "failed to forward packet from channel in {}, dropping connection",
                        log_tag
                    );
                    return;
                }

                from_channel = None;
                transmitted_anything = true;
            } else {
                tokio::select! {
                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(msg)) if !matches!(msg, Message::Close(_)) => {
                                from_network = Some(msg);
                            }
                            _ => {
                                tracing::debug!("websocket closed, dropping channel");
                                return;
                            }
                        }
                    }

                    msg = receiver.recv() => {
                        match msg {
                            Ok(msg) => {
                                from_channel = Some(msg);
                            }
                            _ => {
                                tracing::debug!("tokio select fell through in {}, getting new channel", log_tag);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}
