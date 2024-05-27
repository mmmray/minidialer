use std::sync::RwLock;
use std::{collections::HashMap, sync::Arc};

use anyhow::Error;
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::Response,
    routing::{get, post},
    Router,
};
use futures::task::Poll;
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio::{io::AsyncWriteExt, sync::Mutex};
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
    upload_sockets: Arc<RwLock<HashMap<String, Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>>>>,
}

async fn down_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response<Body> {
    let upstream = match TcpStream::connect(&state.upstream).await {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!("failed to connect to upstream: {e}");
            return Response::builder()
                .status(502)
                .body(Body::from(()))
                .unwrap();
        }
    };

    upstream.set_nodelay(true).unwrap();

    let (upstream_down, upstream_up) = upstream.into_split();

    state
        .upload_sockets
        .write()
        .unwrap()
        .insert(session_id.clone(), Arc::new(Mutex::new(upstream_up)));

    let mut guard = Some(RemoveUploadSocket(state, session_id));

    let body_stream = ReaderStream::new(upstream_down).chain(futures::stream::poll_fn(move |_| {
        let _dropped = guard.take();
        Poll::Ready(None)
    }));

    let body = Body::from_stream(body_stream);

    Response::builder()
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

struct RemoveUploadSocket(AppState, String);

impl Drop for RemoveUploadSocket {
    fn drop(&mut self) {
        self.0.upload_sockets.write().unwrap().remove(&self.1);
    }
}

async fn up_handler(State(state): State<AppState>, Path(session_id): Path<String>, body: Bytes) {
    // on separate line, ensure that we don't hold the lock for too long
    let sender = state
        .upload_sockets
        .read()
        .unwrap()
        .get(&session_id)
        .cloned();

    tracing::debug!("up_handler got {} bytes", body.len());

    if let Some(sender) = sender {
        let mut sender = sender.lock().await;

        if let Err(e) = sender.write_all(&body).await {
            tracing::debug!("failed to write to closed upstream: {e}");
        }
    } else {
        tracing::debug!("could not find session id");
    }
}
