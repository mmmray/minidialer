use std::future::Future;

use async_channel::{bounded, Receiver, Sender};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, Uri},
    response::{Html, Redirect},
    routing::{any, get},
    Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::BrowserCli;

type Pipe = (Sender<Message>, Receiver<Message>);

pub async fn main(args: BrowserCli) {
    let (browser_listen_queue_in, browser_listen_queue_out) = bounded(4096);

    let state = AppState {
        csrf_token: Uuid::new_v4().to_string(),
        browser_listen_queue_in,
        browser_listen_queue_out,
        upstream: args.upstream.clone(),
    };

    let app = Router::new()
        .route("/minidialer/", get(root))
        .route(
            "/minidialer",
            get(|| async { Redirect::temporary("/minidialer/") }),
        )
        .route(
            "/minidialer/dialer.js",
            get(|| async {
                (
                    [("Content-Type", "application/javascript")],
                    include_str!("../static/dialer.js"),
                )
            }),
        )
        .route(
            "/minidialer/socket",
            any(|state, params, headers, ws: WebSocketUpgrade| async {
                ws.on_upgrade(|ws| browser_handler(state, params, headers, ws))
            }),
        )
        .fallback(|state, uri, ws: WebSocketUpgrade| async {
            ws.on_upgrade(|ws| client_handler(state, uri, ws))
        })
        .with_state(state);

    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream);
    tracing::info!(
        "open http://{}/minidialer/ in a browser, and connect to ws://{} instead of {}",
        addr,
        addr,
        args.upstream
    );

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[derive(Deserialize)]
struct Params {
    csrf: String,
}

#[derive(Clone)]
struct AppState {
    csrf_token: String,
    upstream: String,
    browser_listen_queue_in: Sender<Pipe>,
    browser_listen_queue_out: Receiver<Pipe>,
}

async fn root(State(state): State<AppState>) -> Html<String> {
    Html(format!(
        "<!DOCTYPE html><script src=dialer.js></script><script>dialMain(\"{}\")</script>",
        state.csrf_token
    ))
}

async fn browser_handler(
    State(state): State<AppState>,
    Query(params): Query<Params>,
    headers: HeaderMap,
    socket: WebSocket,
) {
    if state.csrf_token != params.csrf {
        tracing::warn!(
            "origin {:?} presented a mismatched csrf token. refresh browser page?",
            headers.get("origin")
        );
        return;
    }

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

async fn client_handler(State(state): State<AppState>, uri: Uri, socket: WebSocket) {
    let get_pipe = || async {
        loop {
            let (sender, receiver) = state.browser_listen_queue_out.recv().await.unwrap();
            tracing::debug!(
                "used browser, now idle: {}",
                state.browser_listen_queue_out.len()
            );

            let dialer_url = format!(
                "{}{}",
                state.upstream,
                uri.path_and_query()
                    .map(|x| x.as_str())
                    .unwrap_or_else(|| uri.path())
            );

            tracing::debug!("dialing {}", dialer_url);

            let Ok(_) = sender.send(Message::Text(dialer_url)).await else {
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
