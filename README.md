# minidialer

A proxy application for obfuscating TLS fingerprints in multiple ways. Designed
for use with v2ray variants.

**This is an experiment and has not been deployed in the field.**

## Installation

```
git clone https://github.com/mmmray/minidialer
cd minidialer
cargo build --release
```

If you are on Windows or do not have curl installed, you can instead run:

```
cargo build --release --no-default-features
```

Binary is in `./target/release/minidialer`

Or, for development, use `cargo run --` instead of `minidialer` command.

## Browser Dialer (websocket-only)

Open a webpage in a browser to use that browser's TLS stack. Only works for
websocket-based v2ray configs.

The browser dialer is very similar to Xray's [Browser Dialer](https://xtls.github.io/en/config/features/browser_dialer.html), and v2fly's [Browser Forwarder](https://www.v2fly.org/en_US/v5/config/service/browser.html)

In fact the code on the client-side ended up very similar to Xray, however for
some reason minidialer seems to have much higher throughput on `speedtest.net`
than xray's dialer. I have not tested v2fly.

Requirements:

* Have an existing `ws`-based v2ray setup, such as `ws+vless` or `ws+vmess`.
  For this example, we assume that the client connects to
  `wss://example.com/mypath` (meaning `path=/mypath`,
  `server=example.com:443`, TLS enabled)
* **The server needs to speak TLS (WSS)** -- if it only speaks plain HTTP/WS,
  there is no point in using minidialer. REALITY/VISION/XTLS and other
  non-standard TLS variants do not work.

Steps:

1. Run `minidialer browser wss://example.com`
2. Change the v2ray client to connect to `ws://localhost:3000/mypath` instead
   of `wss://example.com/mypath`. **Turn off TLS on the client**, it will be
   added by minidialer.
3. Open a browser to `http://localhost:3000/minidialer/`, for example:

   ```
   chromium-browser --headless=new http://localhost:3000/minidialer/
   ```

As a result, the traffic flow changes from this:

```
apps -> v2ray-client -> v2ray-server
```

to this:

```
apps -> v2ray-client -> minidialer -> browser -> v2ray-server
```

Make sure that `browser` is not routed to `v2ray-client` like other `apps`!
System proxy is a problem.

## Command dialer (any TCP)

This is designed to use `openssl s_client` to add TLS. This is useful because
`s_client` is a standard tool with many command-line parameters to tweak
ciphersuites and other things contributing to fingerprints. It does not have to
be `openssl`, it can be any script that uses stdin/stdout for traffic.

**Note**: minidialer does very little here other than spawning the given
command per TCP connection, and can probably be replaced with `socat` entirely.
I haven't gotten it to run reliably though, so I wrote this instead.

Requirements:

* There is an existing TCP-based tunnel wrapped in TLS. The protocol inside TLS
  does not matter. It can be something other than HTTP or WebSockets.
* Server speaks TLS.

Steps:

1. Run: `minidialer command -- openssl s_client -quiet -verify_quiet -verify_return_error example.com:443`
2. Point your v2ray client to connect to `localhost:3000` instead of `example.com:443`, and turn off TLS.
3. Tweak the `openssl` command to change the fingerprint, for example add `-cipher TLS_AES_128_GCM_SHA256,TLS_AES_256_GCM_SHA384,TLS_CHACHA20_POLY1305_SHA256,ECDHE-ECDSA-AES128-GCM-SHA256,ECDHE-RSA-AES128-GCM-SHA256,ECDHE-ECDSA-AES256-GCM-SHA384,ECDHE-RSA-AES256-GCM-SHA384,ECDHE-ECDSA-CHACHA20-POLY1305,ECDHE-RSA-CHACHA20-POLY1305,ECDHE-RSA-AES128-SHA,ECDHE-RSA-AES256-SHA,AES128-GCM-SHA256,AES256-GCM-SHA384,AES128-SHA,AES256-SHA`
4. Or switch to `boringssl`, or to a script that randomly switches between multiple commands.

## Curl WebSocket Dialer

The curl dialer is a websocket reverse proxy that uses curl's experimental
websocket support to connect to the server.

This can be used to manipulate the TLS fingerprint using
[curl-impersonate](https://github.com/lwthiker/curl-impersonate).

Requirements:

* Have a websocket tunnel at `wss://example.com/mypath`

Steps:

1. Run `minidialer curl-ws wss://example.com`
2. Change v2ray to connect to `ws://localhost:3000` instead of `wss://example.com`
3. To actually obfuscate the fingerprint, use [`LD_PRELOAD` to inject curl-impersonate](https://github.com/lwthiker/curl-impersonate?tab=readme-ov-file#using-curl_impersonate-env-var):

   ```
   export RUST_LOG=debug  # to see some noise on console
   export LD_PRELOAD=$HOME/Downloads/libcurl-impersonate-chrome.so  # download from https://github.com/lwthiker/curl-impersonate/releases
   export CURL_IMPERSONATE=chrome116  # see https://github.com/lwthiker/curl-impersonate?tab=readme-ov-file#supported-browsers for possible values
   target/release/minidialer curl wss://example.com
   ```

## Curl TCP Dialer

The curl TCP dialer is similar to the curl WebSocket dialer, except it is a TCP
reverse proxy that uses curl only for establishing a TLS connection.

The inner payload, be it simple HTTP, WebSocket, raw VLESS or any other
protocol, is transmitted as-is. This means that HTTP headers such as User-Agent
are not rewritten using `curl-impersonate`.

Requirements:

* Have some kind of TCP-based server.

Steps:

1. Run `minidialer curl-tcp example.com:443`
2. Change v2ray to connect to `ws://localhost:3000` instead of
   `wss://example.com` (in case of websocket, adapt for other protocols)
3. Follow the Curl WebSocket Dialer docs to customize the TLS fingerprint.

## TCP Fragment

A tool to inject TCP fragmentation at user-defined locations.

This can be used to fragment SNI and Hostname in very specific locations in
attempts to trick DPI.

Requirements:

* Have an existing cloudflare setup, with a domain like `example.com`.
* Have cloudflare configured to accept the same requests on a subdomain
  `www.speedtest.net.example.com`. Wildcard subdomains work too.

Steps:

1. Run `minidialer tcp-fragment --split-after www.speedtest.net www.speedtest.net:80`
2. Change your cloudflare config to talk to `localhost:3000` (without TLS), and change request host and SNI to `www.speedtest.net.example.com`

Remarks:

* The dialer connects to the real `www.speedtest.net` IP address, and, using
  packet fragmentation, tricks DPI into thinking that the hostname
  `www.speedtest.net` is being accessed, while cloudflare sees
  `www.speedtest.net.example.com`.

  `--split-after www.speedtest.net` means that `minidialer` will fragment after
  encountering the string `www.speedtest.net` in the TCP stream, and pause
  transmission for 5 seconds. This causes DPI to assume the wrong hostname
  `www.speedtest.net`, even though it is continued later in another packet.

* 5 seconds can be changed with `--split-sleep-ms` to something else. A high
  value is necessary to trick the GFW, but a low value is desirable for fast
  connection. It is recommended to find the right value using trial-and-error,
  and to compensate for the degraded connection experience using MUX.

* The above example works with plaintext HTTP and `Host` header, but it can be done with SSL and (plaintext!) SNI. The
  issue with SSL is that certificates for multi-level subdomains
  `a.b.c.example.com` are not part of the free Cloudflare offering, and are
  instead in a paid addon called [Total
  TLS](https://developers.cloudflare.com/ssl/edge-certificates/additional-options/total-tls/error-messages/)

## Split HTTP tunnel

The "split http" tunnel is a tool to proxy TCP streams through CDNs without
WebSocket support. The only requirement are working streaming HTTP responses.
Uploads are implemented as separate HTTP requests.

The proxied TCP session is terminated when the streaming HTTP response is
terminated.

For usage, run each command in a separate terminal:

```
nc -l 8080  # our actual TCP-based server
minidialer split-http-server localhost:8080
minidialer split-http --port 3001 http://localhost:3000
nc localhost 3001
```

Now a bidirectional connection is established between first and last netcat.

```
nc -l 8080 <-> split-http-server <-> split-http <-> nc client
```


## Future ideas

* Integrate chromium network stack or other ideas from naiveproxy -- should be
  easier than in v2ray because it's not Golang
* Port performance improvements to xray's browser dialer... once I have figured
  out _why_ minidialer is faster.
* Provide docker container with headless chrome, `node` and `openssl` bundled.

## License

Licensed under `MIT`, see `./LICENSE`
