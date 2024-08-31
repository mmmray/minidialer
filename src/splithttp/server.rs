use std::cmp::Ordering;
use std::collections::BinaryHeap;
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

struct UploadSocket {
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
        self.seq.cmp(&other.seq)
    }
}

impl PartialEq for Packet {
    fn eq(&self, other: &Packet) -> bool {
        self.seq == other.seq
    }
}

impl Eq for Packet {}

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

    let upload_socket = UploadSocket {
        raw_writer: upstream_up,
        next_seq: 0,
        packet_queue: BinaryHeap::new(),
    };

    state
        .upload_sockets
        .write()
        .unwrap()
        .insert(session_id.clone(), Arc::new(Mutex::new(upload_socket)));

    let mut guard = Some(RemoveUploadSocket(state, session_id));

    let body_stream = ReaderStream::new(upstream_down).chain(futures::stream::poll_fn(move |_| {
        let _dropped = guard.take();
        Poll::Ready(None)
    }));

    let body = Body::from_stream(body_stream);

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

async fn up_handler(
    State(state): State<AppState>,
    Path((session_id, seq)): Path<(String, u64)>,
    body: Bytes,
) {
    // on separate line, ensure that we don't hold the lock for too long
    let sender = state
        .upload_sockets
        .read()
        .unwrap()
        .get(&session_id)
        .cloned();

    tracing::debug!("up_handler got {} bytes", body.len());

    let Some(sender) = sender else {
        tracing::debug!("could not find session id");
        return;
    };

    let mut upload_socket = sender.lock().await;

    upload_socket.packet_queue.push(Packet { data: body, seq });

    loop {
        {
            let Some(peeked) = upload_socket.packet_queue.peek() else {
                break;
            };
            if peeked.seq > upload_socket.next_seq {
                break;
            }
        }

        let packet = upload_socket.packet_queue.pop().unwrap();
        if packet.seq == upload_socket.next_seq {
            if let Err(e) = upload_socket.raw_writer.write_all(&packet.data).await {
                tracing::debug!("failed to write to closed upstream: {e}");
            }
        }

        upload_socket.next_seq = packet.seq + 1;
    }
}
