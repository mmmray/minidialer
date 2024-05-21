FROM rust:1.78-bookworm as builder

RUN apt-get update && apt-get install -y libcurl4-openssl-dev

RUN USER=root cargo new --bin /minidialer
WORKDIR /minidialer

COPY Cargo.toml .
COPY Cargo.lock .
RUN cargo build --release && rm -rf src/

COPY . ./
RUN rm ./target/release/deps/minidialer* && cargo build --release

FROM debian:bookworm

RUN apt-get update && apt-get install -y libcurl4-openssl-dev

COPY --from=builder /minidialer/target/release/minidialer /usr/local/bin/minidialer

ENTRYPOINT ["minidialer"]
