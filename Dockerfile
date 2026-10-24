# restrust server + web client image (published as mahmoudetc/restrust).
# Multi-stage: builds the trunk WASM client, the axum server, then ships both
# in a slim runtime. The server serves the client at / (RESTRUST_WEB_DIR).

# --- stage 1: browser client (WASM via Trunk) ---------------------------------
FROM rust:1-bookworm AS web
RUN apt-get update \
    && apt-get install -y --no-install-recommends binaryen \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Manifests first: the toolchain below must honor rust-toolchain.toml.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk --locked --version 0.22.0
# Every workspace member is copied (the root manifest references them all);
# only the client is built.
COPY crates crates
COPY client client
COPY server server
RUN cd client && trunk build --release --locked

# --- stage 2: sync server -------------------------------------------------------
FROM rust:1-bookworm AS server
WORKDIR /src
# The client sources are copied too (the root manifest declares it as a
# workspace member); only the server package is built.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY client client
COPY server server
RUN cargo build --release --locked -p restrust-server

# --- stage 3: runtime ------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home /data --shell /usr/sbin/nologin restrust \
    && mkdir -p /data \
    && chown restrust:restrust /data
COPY --from=server /src/target/release/restrust-server /usr/local/bin/restrust-server
COPY --from=web /src/client/dist /srv/restrust/web
USER restrust
WORKDIR /data
ENV RESTRUST_BIND_ADDR=0.0.0.0 \
    RESTRUST_HTTP_PORT=8080 \
    RESTRUST_DATA_DIR=/data \
    RESTRUST_WEB_DIR=/srv/restrust/web
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/restrust-server"]
