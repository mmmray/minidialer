use std::io;

use libc::size_t;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use anyhow::{Context, Error};

use crate::{
    curl::{bindings, check_err, curl_connect_only, curl_get_async_socket},
    CurlTcpCli,
};

pub async fn main(args: CurlTcpCli) {
    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {:?}", addr, args.upstream);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let upstream = if !args.no_tls {
        format!("wss://{}", args.upstream)
    } else {
        tracing::warn!("--no-tls is passed, never deploy this into the wild!");
        format!("ws://{}", args.upstream)
    };

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let upstream = upstream.clone();
        tokio::spawn(async move {
            if let Err(e) = process_connection(socket, upstream).await {
                tracing::warn!("closed connection: {:?}", e);
            }
        });
    }
}

async fn process_connection(mut socket: TcpStream, upstream: String) -> Result<(), Error> {
    let curl_client = curl_connect_only(&upstream, 1).context("curl_connect_only failed")?;
    let curl_socket = curl_get_async_socket(&curl_client);

    let mut buffer: [u8; 2048] = [0; 2048];
    let mut to_curl_send = 0;
    let mut to_client_send = 0;

    loop {
        if to_client_send > 0 {
            tracing::debug!("sending {} bytes to client", to_client_send);
            socket
                .write_all(&buffer[..to_client_send])
                .await
                .context("socket send failed")?;
            to_client_send = 0;
        } else if to_curl_send > 0 {
            tracing::debug!("sending {} bytes to curl", to_curl_send);
            let mut send_buffer = &mut buffer[..to_curl_send];

            while send_buffer.len() > 0 {
                let mut sent: size_t = 0;
                tracing::debug!("curl_easy_send");
                check_err(unsafe {
                    bindings::curl_easy_send(
                        curl_client.0,
                        send_buffer.as_mut_ptr(),
                        send_buffer.len(),
                        (&mut sent) as *mut _,
                    )
                })
                .context("curl_easy_send failed")?;

                send_buffer = &mut send_buffer[sent..];
            }

            to_curl_send = 0;
        } else {
            let mut bytes_received: size_t = 0;

            tracing::debug!("curl_easy_recv");
            // XXX: this hangs after the connection is terminated by the server
            // ideally it would error, like curl_easy_send
            let res = unsafe {
                // nonblocking
                bindings::curl_easy_recv(
                    curl_client.0,
                    (&mut buffer) as *mut _,
                    buffer.len(),
                    (&mut bytes_received) as *mut _,
                )
            };

            check_err(res).context("curl_easy_recv failed")?;

            if bytes_received > 0 {
                to_client_send = bytes_received;
            } else if res == bindings::CURLE_AGAIN {
                // sometimes curl returns no data to read but does not return EAGAIN. in this case
                // we still should do something other than spinning on curl_easy_recv
                tracing::debug!("curl_easy_recv res = {}", res);

                tracing::debug!("selecting");
                tokio::select! {
                    guard = curl_socket.readable() => {
                        tracing::debug!("select: curl socket ready");
                        guard.unwrap().clear_ready() },
                    readable_res = socket.readable() => {
                        tracing::debug!("select: client socket ready");
                        readable_res.context("selecting client socket failed")?;
                        to_curl_send = match socket.try_read(&mut buffer) {
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => 0,
                            Ok(0) => {
                                tracing::debug!("received zero-read after readiness on client socket, closing");
                                return Ok(());
                            }
                            res => res.context("socket read failed")?
                        };
                        tracing::debug!("read {} bytes from client socket", to_curl_send);
                    }
                }
            } else {
                return Ok(());
            }
        }
    }
}
