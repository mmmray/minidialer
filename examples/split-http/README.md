# split-http basic example

This directory contains a docker-compose file with Apache2 and minidialer. The
purpose is to demonstrate how minidialer can tunnel arbitrary TCP connections
through a HTTP reverse proxy.

Apache2 requires at least the following configuration:

* modules `mod_proxy`, `mod_proxy_http` and `mod_rewrite` enabled.
* `AllowOverride All` (in other words, support for `.htaccess` enabled)

Run:

```
docker-compose up -d
curl --connect-to ::127.0.0.1:3000 http://example.com
```

If the tunnel succeeds, you will get the same output as from `curl http://example.com`.

The traffic flow is like this:

```
curl -> minidialer client -> apache2 -> .htaccess RewriteRule -> minidialer server -> example.com
```

In a more realistic setup, the `minidialer server` would be configured to talk
to a v2ray instance, running something simple like `VLESS+TCP`.

This example focuses a lot on Apache2, but the same idea can be applied to
other HTTP reverse proxies, and CDNs that do not support WebSocket support.
