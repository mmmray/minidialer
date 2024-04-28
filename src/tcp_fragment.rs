use std::{cmp, time::Duration};

use anyhow::{Context, Error};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
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
            tracing::debug!("new connection");
            let upstream = match TcpStream::connect(&args.upstream).await {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!("failed to open connection: {:?}", e);
                    return;
                }
            };

            if let Err(e) = process_connection(
                socket,
                upstream,
                args.split_after.as_bytes(),
                args.split_sleep_ms,
            )
            .await
            {
                tracing::warn!("connection closed, error: {:?}", e);
            }
        });
    }
}

async fn process_connection<D, U>(
    mut downstream: D,
    mut upstream: U,
    split_after: &[u8],
    split_sleep_ms: u64,
) -> Result<usize, Error>
where
    D: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let mut upstream_buffer = Box::new([0u8; 65536]);
    let mut downstream_buffer = Box::new([0u8; 65536]);
    let mut downstream_match_offset = 0;
    let mut sleep_count = 0;

    let mut do_sleep = || {
        if cfg!(test) {
            sleep_count += 1;
        }
        tracing::debug!("sleeping");
        sleep(Duration::from_millis(split_sleep_ms))
    };

    'main: loop {
        tokio::select! {
            upstream_read = upstream.read(&mut *upstream_buffer) => {
                let upstream_read = upstream_read.context("failed to read from upstream")?;
                if upstream_read == 0 {
                    tracing::debug!("empty read from upstream");
                    break 'main;
                }

                downstream.write_all(&upstream_buffer[..upstream_read]).await.context("failed to write to downstream")?;
            }
            downstream_read = downstream.read(&mut *downstream_buffer) => {
                let downstream_read = downstream_read.context("failed to read from downstream")?;

                if downstream_read == 0 {
                    tracing::debug!("empty read from downstream");
                    break 'main;
                }

                // just to be sure we will never double-read data
                let downstream_buffer = &downstream_buffer[..downstream_read];

                let buffer_match_prefix = &downstream_buffer[..cmp::min(split_after.len() - downstream_match_offset, downstream_read)];

                if split_after[downstream_match_offset..].starts_with(buffer_match_prefix) {
                    tracing::debug!("found split match at beginning of buffer");

                    downstream_match_offset += buffer_match_prefix.len();

                    upstream.write_all(buffer_match_prefix).await.context("failed to write to upstream")?;

                    if downstream_match_offset == split_after.len() {
                        do_sleep().await;
                        downstream_match_offset = 0;
                    }

                    upstream.write_all(&downstream_buffer[buffer_match_prefix.len()..]).await.context("failed to write to upstream")?;
                } else if let Some(mut idx) = downstream_buffer.windows(split_after.len()).position(|window| window == split_after) {
                    downstream_match_offset = 0;

                    tracing::debug!("found split match in the middle of buffer");

                    idx += split_after.len();

                    upstream.write_all(&downstream_buffer[..idx]).await.context("failed to write to upstream")?;
                    do_sleep().await;
                    upstream.write_all(&downstream_buffer[idx..]).await.context("failed to write to upstream")?;
                } else {
                    for overlap in (1..cmp::min(downstream_buffer.len(), split_after.len()) - 1).rev() {
                        if &downstream_buffer[(downstream_buffer.len() - overlap)..] == &split_after[..overlap] {
                            tracing::debug!("found split match at end of buffer, of length {}", overlap);
                            downstream_match_offset = overlap;
                            upstream.write_all(downstream_buffer).await.context("failed to write to upstream")?;
                            continue 'main;
                        }
                    }

                    tracing::debug!("found no match");
                    upstream.write_all(downstream_buffer).await.context("failed to write to upstream")?;
                }
            }
        }
    }

    Ok(sleep_count)
}

#[cfg(test)]
mod tests {
    use std::{
        fmt, io,
        pin::Pin,
        task::{Context, Poll},
    };

    use tokio::io::{join, ReadBuf};
    use tracing_test::traced_test;

    use super::*;

    /// An AsyncRead that always returns pending
    struct Nothing;

    impl AsyncRead for Nothing {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    /// An AsyncWrite that makes each individual write, and how they are fragmented, visible for
    /// inspection.
    #[derive(Default)]
    struct Fragments(Vec<Vec<u8>>);

    impl PartialEq<Vec<Vec<u8>>> for Fragments {
        fn eq(&self, other: &Vec<Vec<u8>>) -> bool {
            self.0 == *other
        }
    }

    impl fmt::Debug for Fragments {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
            writeln!(f, "Fragments([")?;
            for fragment in &self.0 {
                writeln!(f, "  {:?},", String::from_utf8_lossy(fragment))?;
            }

            writeln!(f, "])")
        }
    }

    impl AsyncRead for Fragments {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if let Some(next) = self.0.first_mut() {
                assert!(!next.is_empty());
                let mut next2 = next.as_slice();
                let rv = std::pin::pin!(&mut next2).poll_read(cx, buf);
                if next2.is_empty() {
                    self.0.remove(0);
                } else {
                    *next = next2.to_vec();
                }
                assert!(matches!(rv, Poll::Ready(Ok(()))));
                return rv;
            }

            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Fragments {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.0.push(buf.to_vec());
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn test_split_begin() {
        let mut downloaded = Vec::new();
        let mut client = join(b"www.speedtest.net.example.com".as_slice(), &mut downloaded);

        let mut uploaded = Fragments::default();
        let mut server = join(Nothing, &mut uploaded);

        process_connection(&mut client, &mut server, b"www.speedtest.net", 0)
            .await
            .unwrap();

        assert_eq!(downloaded, b"");
        assert_eq!(
            uploaded,
            vec![b"www.speedtest.net".to_vec(), b".example.com".to_vec(),]
        );
    }

    #[tokio::test]
    async fn test_split_middle() {
        let mut downloaded = Vec::new();
        let mut client = join(
            b"Host: www.speedtest.net.example.com".as_slice(),
            &mut downloaded,
        );

        let mut uploaded = Fragments::default();
        let mut server = join(Nothing, &mut uploaded);

        process_connection(&mut client, &mut server, b"www.speedtest.net", 0)
            .await
            .unwrap();

        assert_eq!(downloaded, b"");
        assert_eq!(
            uploaded,
            vec![
                b"Host: www.speedtest.net".to_vec(),
                b".example.com".to_vec(),
            ]
        );
    }

    #[tokio::test]
    async fn test_split_partial_end() {
        let mut downloaded = Vec::new();
        let mut client = join(b"Host: www.speedtes".as_slice(), &mut downloaded);

        let mut uploaded = Fragments::default();
        let mut server = join(Nothing, &mut uploaded);

        let sleep_count = process_connection(&mut client, &mut server, b"www.speedtest.net", 0)
            .await
            .unwrap();

        assert_eq!(downloaded, b"");
        assert_eq!(uploaded, vec![b"Host: www.speedtes".to_vec(),]);
        assert_eq!(sleep_count, 0);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_split_partial_end2() {
        let mut downloaded = Vec::new();
        let mut client = join(
            Fragments(vec![
                b"Host: www.speedtes".to_vec(),
                b"t.net.example.com".to_vec(),
            ]),
            &mut downloaded,
        );

        let mut uploaded = Fragments::default();
        let mut server = join(Nothing, &mut uploaded);

        let sleep_count = process_connection(&mut client, &mut server, b"www.speedtest.net", 0)
            .await
            .unwrap();

        assert_eq!(downloaded, b"");
        assert_eq!(
            uploaded,
            vec![
                b"Host: www.speedtes".to_vec(),
                b"t.net".to_vec(),
                b".example.com".to_vec(),
            ]
        );
        assert_eq!(sleep_count, 1);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_split_three() {
        let mut downloaded = Vec::new();
        let mut client = join(
            Fragments(vec![
                b"Host: www.".to_vec(),
                b"speedtes".to_vec(),
                b"t.net.example.com".to_vec(),
            ]),
            &mut downloaded,
        );

        let mut uploaded = Fragments::default();
        let mut server = join(Nothing, &mut uploaded);

        let sleep_count = process_connection(&mut client, &mut server, b"www.speedtest.net", 0)
            .await
            .unwrap();

        assert_eq!(downloaded, b"");
        assert_eq!(
            uploaded,
            vec![
                b"Host: www.".to_vec(),
                b"speedtes".to_vec(),
                b"t.net".to_vec(),
                b".example.com".to_vec(),
            ]
        );
        assert_eq!(sleep_count, 1);
    }
}
