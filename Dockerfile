FROM rust:1.78-bookworm as builder

RUN apt-get update && apt-get install -y libcurl4-openssl-dev

RUN USER=root cargo new --bin /minidialer
WORKDIR /minidialer

COPY Cargo.toml .
COPY Cargo.lock .
RUN cargo build --release && rm -rf src/

COPY . ./
RUN rm ./target/release/deps/minidialer* && cargo build --release

RUN mkdir /curl-impersonate
WORKDIR /curl-impersonate
RUN curl -Lf https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/libcurl-impersonate-v0.6.1.x86_64-linux-gnu.tar.gz | tar xzf -

FROM debian:bookworm

RUN apt-get update && apt-get install -y libcurl4-openssl-dev tini

COPY --from=builder /minidialer/target/release/minidialer /usr/local/bin/minidialer
COPY --from=builder /curl-impersonate /curl-impersonate

ENTRYPOINT ["tini", "--", "minidialer"]
