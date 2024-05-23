use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Error};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::Response,
    routing::{get, post},
    Router,
};
use tokio::io::{
    copy, duplex, split, AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, WriteHalf,
};
use tokio::net::TcpStream;
use tokio_util::io::ReaderStream;

use crate::SplitHttpServerCli;

pub async fn main(args: SplitHttpServerCli) -> Result<(), Error> {
    let state = AppState {
        upstream: args.upstream.clone(),
        upload_sockets: Default::default(),
    };

    let app = Router::new()
        .route("/:session/down", get(down_handler))
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
    upload_sockets: Arc<Mutex<HashMap<String, WriteHalf<DuplexStream>>>>,
}

async fn down_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response<Body> {
    let (downstream_client, downstream_server) = duplex(120 * 1024);

    let (client_downloader, client_uploader) = split(downstream_client);

    state
        .upload_sockets
        .lock()
        .unwrap()
        .insert(session_id.clone(), client_uploader);

    tokio::spawn(async move {
        if let Err(e) = forward_channels(state.clone(), downstream_server).await {
            tracing::debug!("connection closed, error: {:?}", e);
        }

        state.upload_sockets.lock().unwrap().remove(&session_id);
    });

    let body = Body::from_stream(ReaderStream::new(client_downloader));
    Response::builder()
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

async fn forward_channels<D>(state: AppState, downstream: D) -> Result<(), Error>
where
    D: AsyncRead + AsyncWrite + Unpin,
{
    let upstream = TcpStream::connect(&state.upstream)
        .await
        .context("failed to connect to upstream")?;

    let (mut downstream_up, mut downstream_down) = split(downstream);
    let (mut upstream_down, mut upstream_up) = split(upstream);

    // copy_bidirectional does not work here, because it hangs when one side is still open. we want
    // to terminate when either side closes.

    tokio::select! {
        _ = copy(&mut downstream_up, &mut upstream_up) => {}
        _ = copy(&mut upstream_down, &mut downstream_down) => {}
    };

    Ok(())
}

async fn up_handler(State(state): State<AppState>, Path(session_id): Path<String>, body: Bytes) {
    // on separate line, ensure that we don't hold the lock for too long
    let sender = state.upload_sockets.lock().unwrap().remove(&session_id);

    if let Some(mut sender) = sender {
        tracing::debug!("up_handler got {} bytes", body.len());
        sender.write_all(&body).await.unwrap();
        state
            .upload_sockets
            .lock()
            .unwrap()
            .insert(session_id, sender);
    } else {
        tracing::debug!("could not find session id");
    }
}
