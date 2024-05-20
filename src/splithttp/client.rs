use anyhow::{Context, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::SplitHttpCli;

pub async fn main(args: SplitHttpCli) -> Result<(), Error> {
    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream,);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let upstream_client = reqwest::Client::new();

    loop {
        let (socket, _) = listener.accept().await.unwrap();

        let args = args.clone();

        let upstream_client = upstream_client.clone();
        let upstream = args.upstream.clone();

        tokio::spawn(async move {
            tracing::debug!("new connection");

            if let Err(e) = process_connection(socket, upstream_client, upstream).await {
                tracing::warn!("connection closed, error: {:?}", e);
            }
        });
    }
}

async fn process_connection(
    mut downstream: TcpStream,
    upstream_client: reqwest::Client,
    upstream: String,
) -> Result<(), Error> {
    let session_id = uuid::Uuid::new_v4();

    let mut downloader = upstream_client
        .post(format!("{upstream}/{session_id}/down"))
        .send()
        .await?;

    let mut downstream_buffer = Box::new([0u8; 65536]);

    loop {
        tokio::select! {
            upstream_read = downloader.chunk() => {
                let upstream_read = upstream_read.context("failed to read from upstream")?;

                if let Some(upstream_read) = upstream_read {
                    downstream.write_all(&*upstream_read).await.context("failed to write to downstream")?;
                } else {
                    tracing::debug!("empty read from upstream");
                    return Ok(());
                }
            }

            downstream_read = downstream.read(&mut *downstream_buffer) => {
                let downstream_read = downstream_read.context("failed to read from downstream")?;

                if downstream_read == 0 {
                    tracing::debug!("empty read from downstream");
                    return Ok(());
                }

                let response = upstream_client.post(format!("{upstream}/{session_id}/up")).body(downstream_buffer[..downstream_read].to_vec()).send().await.context("failed to write to upstream")?;
                response.error_for_status()?;
            }
        }
    }
}
