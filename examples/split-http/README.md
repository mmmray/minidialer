# split-http basic example

This directory contains a docker-compose file with Apache2 and minidialer. The
purpose is to demonstrate how minidialer can tunnel arbitrary TCP connections
through a Apache2 host with `mod_proxy` enabled.

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
