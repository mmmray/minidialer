use std::{
    ffi::{CStr, CString},
    ptr::null_mut,
};

use anyhow::{Context, Error};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::Uri;
use axum::Router;
use libc::size_t;
use tokio::io::unix::AsyncFd;

use crate::CurlCli;

#[allow(bad_style)]
mod bindings {
    use libc::{c_char, size_t};

    pub type __enum_ty = libc::c_uint;

    pub type CURLcode = __enum_ty;
    pub type CURLoption = __enum_ty;

    pub const CURLOPTTYPE_LONG: CURLoption = 0;
    pub const CURLOPTTYPE_OBJECTPOINT: CURLoption = 10_000;
    pub const CURLOPT_URL: CURLoption = CURLOPTTYPE_OBJECTPOINT + 2;
    pub const CURLOPT_CONNECT_ONLY: CURLoption = CURLOPTTYPE_LONG + 141;
    pub const CURLE_OK: CURLcode = 0;
    pub const CURLE_AGAIN: CURLcode = 81;
    pub const CURLWS_BINARY: libc::c_uint = 1 << 1;

    pub const CURLINFO_SOCKET: __enum_ty = 0x500000;
    pub const CURLINFO_ACTIVESOCKET: __enum_ty = CURLINFO_SOCKET + 44;
    pub type curl_off_t = i64;
    pub type curl_socket_t = libc::c_int;

    pub enum CURL {}

    // CURL client can be sent across threads but not used concurrently
    pub struct SendableCurl(pub *mut CURL);
    unsafe impl Send for SendableCurl {}

    #[repr(C)]
    pub struct curl_ws_frame {
        age: libc::c_int,      /* zero */
        flags: libc::c_int,    /* See the CURLWS_* defines */
        offset: curl_off_t,    /* the offset of this data into the frame */
        bytesleft: curl_off_t, /* number of pending bytes left of the payload */
        len: size_t,           /* size of the current data chunk */
    }

    // copypasted from curl-sys' lib.rs and partially hand-written, because curl-sys does not have
    // symbols for websocket (curl_ws_..)
    #[link(name = "curl")]
    extern "C" {
        pub fn curl_easy_init() -> *mut CURL;
        #[must_use]
        pub fn curl_easy_setopt(curl: *mut CURL, option: CURLoption, ...) -> CURLcode;
        #[must_use]
        pub fn curl_easy_perform(curl: *mut CURL) -> CURLcode;
        pub fn curl_easy_strerror(code: CURLcode) -> *const c_char;

        #[must_use]
        pub fn curl_ws_send(
            curl: *mut CURL,
            buffer: *const u8,
            buflen: size_t,
            sent: *mut size_t,
            fragsize: curl_off_t,
            flags: libc::c_uint,
        ) -> CURLcode;

        #[must_use]
        pub fn curl_ws_recv(
            curl: *mut CURL,
            buffer: *const u8,
            buflen: size_t,
            recv: *mut size_t,
            meta: *mut *mut curl_ws_frame,
        ) -> CURLcode;

        #[must_use]
        pub fn curl_easy_getinfo(
            handle: *mut CURL,
            info: __enum_ty,
            socket: *mut curl_socket_t,
        ) -> CURLcode;
    }
}

fn check_err(code: bindings::CURLcode) -> Result<(), Error> {
    if code != bindings::CURLE_OK && code != bindings::CURLE_AGAIN {
        let err = unsafe { CStr::from_ptr(bindings::curl_easy_strerror(code)) };
        anyhow::bail!("code {}: {:?}", code, err);
    }

    Ok(())
}

#[derive(Clone)]
struct AppState {
    upstream: String,
}

pub async fn main(args: CurlCli) -> Result<(), Error> {
    let state = AppState {
        upstream: args.upstream.clone(),
    };

    let app = Router::new()
        .fallback(|state, uri, ws: WebSocketUpgrade| async {
            ws.on_upgrade(|ws| curl_handler(state, uri, ws))
        })
        .with_state(state);

    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!("listening on {}, forwarding to {}", addr, args.upstream);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

async fn curl_handler(State(state): State<AppState>, uri: Uri, mut socket: WebSocket) {
    let dialer_url = format!(
        "{}{}",
        state.upstream,
        uri.path_and_query()
            .map(|x| x.as_str())
            .unwrap_or_else(|| uri.path())
    );

    tracing::debug!("connecting to {}", dialer_url);

    let curl_client = unsafe {
        let rv = bindings::curl_easy_init();
        assert!(!rv.is_null());
        let url = CString::new(dialer_url).unwrap();
        check_err(bindings::curl_easy_setopt(
            rv,
            bindings::CURLOPT_URL,
            url.as_ptr(),
        ))
        .unwrap();
        check_err(bindings::curl_easy_setopt(
            rv,
            bindings::CURLOPT_CONNECT_ONLY,
            2 as libc::c_longlong,
        ))
        .unwrap();
        bindings::SendableCurl(rv)
    };
    tracing::debug!("curl initialized");

    if let Err(e) = check_err(unsafe { bindings::curl_easy_perform(curl_client.0) }) {
        tracing::warn!("curl_easy_perform failed: {}", e);
        return;
    }

    let curl_socket = {
        let mut socket: bindings::curl_socket_t = 0;
        let res = unsafe {
            bindings::curl_easy_getinfo(
                curl_client.0,
                bindings::CURLINFO_ACTIVESOCKET,
                (&mut socket) as *mut _,
            )
        };
        check_err(res)
            .context("curl_easy_getinfo(CURLINFO_ACTIVESOCKET) failed")
            .unwrap();
        AsyncFd::new(socket).unwrap()
    };

    let mut to_curl_send: Option<Message> = None;
    let mut to_client_send: Option<Message> = None;

    loop {
        if let Some(to_send) = to_client_send.take() {
            tracing::debug!("sending to client");
            if socket.send(to_send).await.is_err() {
                tracing::debug!("failed to forward data from upstream, dropping connection");
                return;
            }
        } else if let Some(to_send) = to_curl_send.take() {
            tracing::debug!("sending to curl");
            let mut to_send = to_send.into_data();
            let mut send_buffer = to_send.as_mut_slice();

            while send_buffer.len() > 0 {
                let mut sent: size_t = 0;
                tracing::debug!("curl_ws_send");
                let res = unsafe {
                    bindings::curl_ws_send(
                        curl_client.0,
                        send_buffer.as_mut_ptr(),
                        send_buffer.len(),
                        (&mut sent) as *mut _,
                        0,
                        bindings::CURLWS_BINARY,
                    )
                };

                if let Err(e) = check_err(res) {
                    tracing::warn!("curl_ws_send failed: {}", e);
                    return;
                }

                send_buffer = &mut send_buffer[sent..];
            }
        } else {
            let mut bytes_received: size_t = 0;
            let mut buffer: [u8; 256] = [0; 256];

            tracing::debug!("curl_ws_recv");
            // XXX: this hangs after the connection is terminated by the server
            // ideally it would error, like curl_ws_send
            let res = unsafe {
                let mut meta: *mut bindings::curl_ws_frame = null_mut();

                // nonblocking
                bindings::curl_ws_recv(
                    curl_client.0,
                    (&mut buffer) as *mut _,
                    buffer.len(),
                    (&mut bytes_received) as *mut _,
                    (&mut meta) as *mut _,
                )
            };

            if let Err(e) = check_err(res) {
                tracing::warn!("curl_ws_recv failed: {}", e);
                return;
            }

            if bytes_received > 0 {
                to_client_send = Some(Message::Binary(buffer[..bytes_received].to_vec()));
            } else if res == bindings::CURLE_AGAIN {
                tracing::debug!("selecting");
                tokio::select! {
                    guard = curl_socket.readable() => { guard.unwrap().clear_ready() },
                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(msg)) if !matches!(msg, Message::Close(_)) => {
                                if matches!(msg, Message::Ping(_) | Message::Pong(_)) {
                                    tracing::debug!("skipping non-payload message");
                                } else {
                                    tracing::debug!("to_curl_send set to {:?}", msg);
                                    to_curl_send = Some(msg);
                                }
                            }
                            _ => {
                                tracing::debug!("websocket closed, dropping channel");
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}
