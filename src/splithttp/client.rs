use anyhow::{Context, Error};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::SplitHttpCli;

pub async fn main(args: SplitHttpCli) -> Result<(), Error> {
    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream,);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let headermap = parse_header_args(&args.header);
    let download_headermap = if args.download_upstream.is_some() {
        parse_header_args(&args.download_header)
    } else {
        headermap.clone()
    };

    let upstream_client = reqwest::Client::new();

    loop {
        let (socket, _) = listener.accept().await.unwrap();

        let upstream_client = upstream_client.clone();
        let download_upstream = args
            .download_upstream
            .as_ref()
            .unwrap_or(&args.upstream)
            .clone();
        let upstream = args.upstream.clone();
        let download_headermap = download_headermap.clone();
        let headermap = headermap.clone();

        tokio::spawn(async move {
            tracing::debug!("new connection");

            if let Err(e) = process_connection(
                socket,
                upstream_client,
                headermap,
                download_headermap,
                download_upstream,
                upstream,
                args.upload_chunk_size,
            )
            .await
            {
                tracing::warn!("connection closed, error: {:?}", e);
            }
        });
    }
}

fn parse_header_args(cli: &[String]) -> HeaderMap {
    let mut headermap = HeaderMap::new();

    for header in cli {
        let (k, v) = header.split_once(':').unwrap();
        let k = HeaderName::from_bytes(k.as_bytes()).unwrap();
        let v = HeaderValue::from_bytes(v.as_bytes()).unwrap();
        headermap.insert(k, v);
    }

    headermap
}

async fn process_connection(
    mut downstream: TcpStream,
    upstream_client: reqwest::Client,
    headermap: HeaderMap,
    download_headermap: HeaderMap,
    download_upstream: String,
    upstream: String,
    upload_chunk_size: usize,
) -> Result<(), Error> {
    let session_id = uuid::Uuid::new_v4();

    let mut downloader = upstream_client
        .get(format!("{download_upstream}/{session_id}/down"))
        .headers(download_headermap)
        .send()
        .await?
        .error_for_status()?;

    let mut downstream_buffer = vec![0; upload_chunk_size].into_boxed_slice();

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

                let response = upstream_client
                    .post(format!("{upstream}/{session_id}/up"))
                    .headers(headermap.clone())
                    .body(downstream_buffer[..downstream_read].to_vec())
                    .send()
                    .await
                    .context("failed to write to upstream")?;
                response.error_for_status()?;
            }
        }
    }
}
