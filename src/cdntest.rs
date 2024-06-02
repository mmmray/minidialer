use std::time::Duration;

use anyhow::Error;
use axum::{body::Body, http::Response, routing::get, Router};
use tokio::{
    io::{duplex, AsyncWriteExt},
    time::sleep,
};
use tokio_util::io::ReaderStream;

use crate::CdnTestCli;

pub async fn main(args: CdnTestCli) -> Result<(), Error> {
    let app = Router::new().route("/chunked-pong", get(chunked_pong));

    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

async fn chunked_pong() -> Response<Body> {
    let (read, mut write) = duplex(1024);

    tokio::spawn(async move {
        sleep(Duration::from_secs(3)).await;
        while write.write(b"x\n").await.is_ok() {
            sleep(Duration::from_secs(1)).await;
        }
    });

    Response::builder()
        .body(Body::from_stream(ReaderStream::new(read)))
        .unwrap()
}
