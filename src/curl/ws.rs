use std::ptr::null_mut;

use anyhow::Error;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::Uri;
use axum::Router;
use libc::size_t;

use crate::curl::{bindings, check_err, curl_connect_only, curl_get_async_socket};
use crate::CurlWsCli;

#[derive(Clone)]
struct AppState {
    upstream: String,
}

pub async fn main(args: CurlWsCli) -> Result<(), Error> {
    let state = AppState {
        upstream: args.upstream.clone(),
    };

    let app = Router::new()
        .fallback(|state, uri, ws: WebSocketUpgrade| async {
            ws.on_upgrade(|ws| curl_handler(state, uri, ws))
        })
        .with_state(state);

    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}
async fn curl_handler(State(state): State<AppState>, uri: Uri, mut socket: WebSocket) {
    let dialer_url = format!(
        "{}{}",
        state.upstream,
        uri.path_and_query()
            .map(|x| x.as_str())
            .unwrap_or_else(|| uri.path())
    );

    tracing::debug!("connecting to {}", dialer_url);

    let curl_client = match curl_connect_only(&dialer_url, 2) {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!("curl_easy_perform failed: {}", e);
            return;
        }
    };

    let curl_socket = curl_get_async_socket(&curl_client);

    let mut to_curl_send: Option<Message> = None;
    let mut to_client_send: Option<Message> = None;

    loop {
        if let Some(to_send) = to_client_send.take() {
            tracing::debug!("sending to client");
            if socket.send(to_send).await.is_err() {
                tracing::debug!("failed to forward data from upstream, dropping connection");
                return;
            }
        } else if let Some(to_send) = to_curl_send.take() {
            tracing::debug!("sending to curl");
            let mut to_send = to_send.into_data();
            let mut send_buffer = to_send.as_mut_slice();

            while send_buffer.len() > 0 {
                let mut sent: size_t = 0;
                tracing::debug!("curl_ws_send");
                let res = unsafe {
                    bindings::curl_ws_send(
                        curl_client.0,
                        send_buffer.as_mut_ptr(),
                        send_buffer.len(),
                        (&mut sent) as *mut _,
                        0,
                        bindings::CURLWS_BINARY,
                    )
                };

                if let Err(e) = check_err(res) {
                    tracing::warn!("curl_ws_send failed: {}", e);
                    return;
                }

                send_buffer = &mut send_buffer[sent..];
            }
        } else {
            let mut bytes_received: size_t = 0;
            let mut buffer: [u8; 2048] = [0; 2048];

            tracing::debug!("curl_ws_recv");
            // XXX: this hangs after the connection is terminated by the server
            // ideally it would error, like curl_ws_send
            let res = unsafe {
                let mut meta: *mut bindings::curl_ws_frame = null_mut();

                // nonblocking
                bindings::curl_ws_recv(
                    curl_client.0,
                    (&mut buffer) as *mut _,
                    buffer.len(),
                    (&mut bytes_received) as *mut _,
                    (&mut meta) as *mut _,
                )
            };

            if let Err(e) = check_err(res) {
                tracing::warn!("curl_ws_recv failed: {}", e);
                return;
            }

            if bytes_received > 0 {
                to_client_send = Some(Message::Binary(buffer[..bytes_received].to_vec()));
            } else if res == bindings::CURLE_AGAIN {
                tracing::debug!("selecting");
                tokio::select! {
                    guard = curl_socket.readable() => { guard.unwrap().clear_ready() },
                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(msg)) if !matches!(msg, Message::Close(_)) => {
                                if matches!(msg, Message::Ping(_) | Message::Pong(_)) {
                                    tracing::debug!("skipping non-payload message");
                                } else {
                                    tracing::debug!("to_curl_send set to {:?}", msg);
                                    to_curl_send = Some(msg);
                                }
                            }
                            _ => {
                                tracing::debug!("websocket closed, dropping channel");
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}
