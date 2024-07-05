use std::time::Duration;

use anyhow::Error;
use axum::{body::Body, extract::Query, http::Response, routing::get, Router};
use serde::Deserialize;
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

#[derive(Deserialize)]
struct Params {
    content_type: Option<String>,
}

async fn chunked_pong(Query(params): Query<Params>) -> Response<Body> {
    let (read, mut write) = duplex(1024);

    tokio::spawn(async move {
        sleep(Duration::from_secs(3)).await;
        let mut i = 0;
        while write.write(format!("{}<br>\n", i).as_bytes()).await.is_ok() {
            sleep(Duration::from_secs(1)).await;
            i += 1;
        }
    });

    Response::builder()
        .header(
            "Content-Type",
            params.content_type.unwrap_or("text/html".to_owned()),
        )
        .body(Body::from_stream(ReaderStream::new(read)))
        .unwrap()
}
