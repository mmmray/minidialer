use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Error};
use async_channel::{bounded, Receiver, Sender};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::Response,
    routing::post,
    Router,
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::SplitHttpServerCli;

pub async fn main(args: SplitHttpServerCli) -> Result<(), Error> {
    let state = AppState {
        upstream: args.upstream.clone(),
        upload_sockets: Default::default(),
    };

    let app = Router::new()
        .route("/:session/down", post(down_handler))
        .route("/:session/up", post(up_handler))
        .with_state(state);

    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

#[derive(Clone)]
struct AppState {
    upstream: String,
    upload_sockets: Arc<Mutex<HashMap<String, Sender<Vec<u8>>>>>,
}

async fn down_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response<Body> {
    let (down_channel_sender, down_channel_receiver) = bounded(4096);
    let (up_channel_sender, up_channel_receiver) = bounded(4096);

    state
        .upload_sockets
        .lock()
        .unwrap()
        .insert(session_id.clone(), up_channel_sender);

    tokio::spawn(async move {
        if let Err(e) =
            forward_channels(state.clone(), up_channel_receiver, down_channel_sender).await
        {
            tracing::warn!("connection closed, error: {:?}", e);
        }

        state.upload_sockets.lock().unwrap().remove(&session_id);
    });

    let body = Body::from_stream(down_channel_receiver.map(Ok::<_, Error>));
    Response::builder()
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

async fn forward_channels(
    state: AppState,
    up_channel_receiver: Receiver<Vec<u8>>,
    down_channel_sender: Sender<Vec<u8>>,
) -> Result<(), Error> {
    let mut upstream = TcpStream::connect(&state.upstream)
        .await
        .context("failed to connect to upstream")?;
    let mut upstream_buffer = Box::new([0u8; 65536]);

    loop {
        tokio::select! {
            upstream_read = upstream.read(&mut *upstream_buffer) => {
                tracing::debug!("read from upstream");
                let upstream_read = upstream_read.context("failed to read from upstream")?;

                if upstream_read == 0 {
                    tracing::debug!("upstream closed");
                    return Ok(());
                }

                down_channel_sender.send(upstream_buffer[..upstream_read].to_vec()).await.context("failed to send to downstream")?;

            }
            downstream_read = up_channel_receiver.recv() => {
                tracing::debug!("read from downstream");
                if let Ok(downstream_read) = downstream_read {
                    upstream.write_all(&downstream_read).await.context("failed to write to upstream")?;
                } else {
                    tracing::debug!("downstream closed");
                    return Ok(());
                }
            }
        }
    }
}

async fn up_handler(State(state): State<AppState>, Path(session_id): Path<String>, body: Bytes) {
    // on separate line, ensure that we don't hold the lock for too long
    let sender = state
        .upload_sockets
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned();

    if let Some(sender) = sender {
        tracing::debug!("up_handler got {} bytes", body.len());
        sender.send(body.to_vec()).await.unwrap();
    } else {
        tracing::debug!("could not find session id");
    }
}
