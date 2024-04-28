use std::{cmp, time::Duration};

use anyhow::{Context, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::{net::TcpStream, time::sleep};

use crate::TcpFragmentCli;

pub async fn main(args: TcpFragmentCli) {
    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream,);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    loop {
        let (socket, _) = listener.accept().await.unwrap();

        let args = args.clone();

        tokio::spawn(async move {
            if let Err(e) = process_connection(socket, args).await {
                tracing::warn!("connection closed, error: {:?}", e);
            }
        });
    }
}

async fn process_connection(mut downstream: TcpStream, args: TcpFragmentCli) -> Result<(), Error> {
    tracing::debug!("new connection");

    let mut upstream = TcpStream::connect(&args.upstream).await?;

    let mut upstream_buffer = [0u8; 65536];
    let mut downstream_buffer = [0u8; 65536];
    let mut downstream_match_offset = 0;
    let split_after = args.split_after.as_bytes();

    'main: loop {
        tokio::select! {
            upstream_read = upstream.read(&mut upstream_buffer) => {
                let upstream_read = upstream_read.context("failed to read from upstream")?;
                if upstream_read == 0 {
                    tracing::debug!("empty read from upstream");
                    return Ok(());
                }

                downstream.write_all(&upstream_buffer[..upstream_read]).await.context("failed to write to downstream")?;
            }
            downstream_read = downstream.read(&mut downstream_buffer) => {
                let downstream_read = downstream_read.context("failed to read from downstream")?;

                if downstream_read == 0 {
                    tracing::debug!("empty read from downstream");
                    return Ok(());
                }

                // just to be sure we will never double-read data
                let downstream_buffer = &downstream_buffer[..downstream_read];

                let buffer_match_prefix = &downstream_buffer[..cmp::min(split_after.len() - downstream_match_offset, downstream_read)];

                if split_after[downstream_match_offset..].starts_with(buffer_match_prefix) {
                    tracing::debug!("found split match at beginning of buffer");

                    downstream_match_offset += buffer_match_prefix.len();

                    upstream.write_all(buffer_match_prefix).await.context("failed to write to upstream")?;

                    if downstream_match_offset == split_after.len() {
                        tracing::debug!("sleeping");
                        sleep(Duration::from_millis(args.split_sleep_ms)).await;
                        downstream_match_offset = 0;
                    }

                    upstream.write_all(&downstream_buffer[buffer_match_prefix.len()..]).await.context("failed to write to upstream")?;
                } else if let Some(mut idx) = downstream_buffer.windows(split_after.len()).position(|window| window == split_after) {
                    downstream_match_offset = 0;

                    tracing::debug!("found split match in the middle of buffer");

                    idx += split_after.len();

                    upstream.write_all(&downstream_buffer[..idx]).await.context("failed to write to upstream")?;
                    tracing::debug!("sleeping");
                    sleep(Duration::from_millis(args.split_sleep_ms)).await;
                    upstream.write_all(&downstream_buffer[idx..]).await.context("failed to write to upstream")?;
                } else {
                    for overlap in (1..cmp::min(downstream_buffer.len(), split_after.len()) - 1).rev() {
                        if &downstream_buffer[(downstream_buffer.len() - overlap)..] == &split_after[..overlap] {
                            tracing::debug!("found split match at end of buffer, of length {}", overlap);
                            upstream.write_all(&downstream_buffer[..(downstream_buffer.len() - overlap)]).await.context("failed to write to upstream")?;
                            sleep(Duration::from_millis(args.split_sleep_ms)).await;
                            upstream.write_all(&downstream_buffer[(downstream_buffer.len() - overlap)..]).await.context("failed to write to upstream")?;
                            continue 'main;
                        }
                    }

                    tracing::debug!("found no match");
                    upstream.write_all(downstream_buffer).await.context("failed to write to upstream")?;
                }
            }
        }
    }
}
