use std::ffi::{CStr, CString};

use anyhow::{Context, Error};
use tokio::io::unix::AsyncFd;

pub mod tcp;
pub mod ws;

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
        pub fn curl_easy_send(
            curl: *mut CURL,
            buffer: *const u8,
            buflen: size_t,
            n: *mut size_t,
        ) -> CURLcode;

        #[must_use]
        pub fn curl_easy_recv(
            curl: *mut CURL,
            buffer: *const u8,
            buflen: size_t,
            n: *mut size_t,
        ) -> CURLcode;

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

fn curl_connect_only(url: &str, value: usize) -> Result<bindings::SendableCurl, Error> {
    let curl_client = unsafe {
        let rv = bindings::curl_easy_init();
        assert!(!rv.is_null());
        let url = CString::new(url).unwrap();
        check_err(bindings::curl_easy_setopt(
            rv,
            bindings::CURLOPT_URL,
            url.as_ptr(),
        ))
        .unwrap();
        check_err(bindings::curl_easy_setopt(
            rv,
            bindings::CURLOPT_CONNECT_ONLY,
            value as libc::c_longlong,
        ))
        .unwrap();
        bindings::SendableCurl(rv)
    };

    check_err(unsafe { bindings::curl_easy_perform(curl_client.0) })?;

    Ok(curl_client)
}

fn curl_get_async_socket(curl_client: &bindings::SendableCurl) -> AsyncFd<i32> {
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
}
