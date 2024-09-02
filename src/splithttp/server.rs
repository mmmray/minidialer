use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::RwLock;
use std::{collections::HashMap, sync::Arc};

use anyhow::Error;
use axum::{
    body::{Body, Bytes},
    debug_handler,
    extract::{Path, Query, State},
    http::Response,
    routing::{get, post},
    Router,
};
use futures::task::Poll;
use futures::StreamExt;
use serde::Deserialize;
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
        .route("/:session", get(down_handler))
        .route("/:session/:seq", post(up_handler))
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
    upload_sockets: Arc<RwLock<HashMap<String, Arc<Mutex<UploadSocket>>>>>,
}

impl AppState {
    async fn upsert_session(self, session_id: String) -> Result<Arc<Mutex<UploadSocket>>, ()> {
        if let Some(session) = self.upload_sockets.read().unwrap().get(&session_id) {
            return Ok(session.clone());
        }

        let upstream = match TcpStream::connect(&self.upstream).await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("failed to connect to upstream: {e}");
                return Err(());
            }
        };

        upstream.set_nodelay(true).unwrap();
        let (upstream_down, upstream_up) = upstream.into_split();
        let upload_socket = UploadSocket {
            raw_reader: Some(upstream_down),
            raw_writer: upstream_up,
            next_seq: 0,
            packet_queue: BinaryHeap::new(),
        };

        let upload = Arc::new(Mutex::new(upload_socket));
        // gross way to deal with race condition: if since the last time read() was called, the
        // upload queue was inserted, we just use that one
        Ok(self
            .upload_sockets
            .write()
            .unwrap()
            .insert(session_id, upload.clone())
            .unwrap_or(upload))
    }
}

struct UploadSocket {
    // taken by download handler
    raw_reader: Option<tokio::net::tcp::OwnedReadHalf>,
    raw_writer: tokio::net::tcp::OwnedWriteHalf,
    next_seq: u64,
    packet_queue: BinaryHeap<Packet>,
}

struct Packet {
    data: Bytes,
    seq: u64,
}

impl PartialOrd for Packet {
    fn partial_cmp(&self, other: &Packet) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Packet {
    fn cmp(&self, other: &Packet) -> Ordering {
        // inverse order: smallest packets should go first in the heap
        self.seq.cmp(&other.seq).reverse()
    }
}

impl PartialEq for Packet {
    fn eq(&self, other: &Packet) -> bool {
        self.seq == other.seq
    }
}

impl Eq for Packet {}

#[derive(Deserialize)]
struct Params {
    x_padding: Option<String>,
}

#[debug_handler]
async fn down_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<Params>,
) -> Response<Body> {
    let Ok(upload_socket) = state.clone().upsert_session(session_id.clone()).await else {
        return Response::builder()
            .status(502)
            .body(Body::from(()))
            .unwrap();
    };

    let Some(download_reader) = upload_socket.lock().await.raw_reader.take() else {
        tracing::warn!("opened download twice");
        return Response::builder()
            .status(502)
            .body(Body::from(()))
            .unwrap();
    };

    let mut guard = Some(RemoveUploadSocket(state, session_id));

    let body_stream =
        ReaderStream::new(download_reader).chain(futures::stream::poll_fn(move |_| {
            let _dropped = guard.take();
            Poll::Ready(None)
        }));

    let body = if params.x_padding.is_some() {
        Body::from_stream(body_stream)
    } else {
        Body::from_stream(futures::stream::once(async { Ok(Bytes::from("ok")) }).chain(body_stream))
    };

    Response::builder()
        .header("X-Accel-Buffering", "no")
        .header("Content-Type", "text/event-stream")
        .body(body)
        .unwrap()
}

struct RemoveUploadSocket(AppState, String);

impl Drop for RemoveUploadSocket {
    fn drop(&mut self) {
        self.0.upload_sockets.write().unwrap().remove(&self.1);
    }
}

#[debug_handler]
async fn up_handler(
    State(state): State<AppState>,
    Path((session_id, seq)): Path<(String, u64)>,
    body: Bytes,
) -> Response<Body> {
    tracing::debug!("up_handler got {} bytes", body.len());

    let Ok(upload_socket) = state.upsert_session(session_id).await else {
        return Response::builder()
            .status(502)
            .body(Body::from(()))
            .unwrap();
    };

    let mut upload_socket = upload_socket.lock().await;

    upload_socket.packet_queue.push(Packet { data: body, seq });

    loop {
        {
            let Some(peeked) = upload_socket.packet_queue.peek() else {
                break;
            };
            tracing::debug!("peeking packet {}", peeked.seq);
            if peeked.seq > upload_socket.next_seq {
                break;
            }
        }

        let packet = upload_socket.packet_queue.pop().unwrap();
        if packet.seq == upload_socket.next_seq {
            tracing::debug!("sending packet {}", packet.seq);
            if let Err(e) = upload_socket.raw_writer.write_all(&packet.data).await {
                tracing::debug!("failed to write to closed upstream: {e}");
                return Response::builder()
                    .status(502)
                    .body(Body::from(()))
                    .unwrap();
            }

            upload_socket.next_seq += 1;
        }
    }

    Response::builder()
        .status(200)
        .body(Body::from(()))
        .unwrap()
}
