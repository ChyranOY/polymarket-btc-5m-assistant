# Multi-stage build for the polymarket-btc-5m Rust bot.
# Final image is ~50MB: debian:bookworm-slim + ca-certificates + the stripped binary + static/.

# ------- Build stage -------
FROM rust:1-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# Warm the dep cache: copy manifests only, compile a stub, then drop the stub.
# Any edit to Cargo.toml / Cargo.lock busts this layer; edits to src/ do not.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && cargo build --release --bin polymarket-btc-5m \
    && rm -rf src

# Real build.
COPY src ./src
COPY tests ./tests
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --bin polymarket-btc-5m \
    && strip target/release/polymarket-btc-5m

# ------- Runtime stage -------
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/polymarket-btc-5m /app/bot
COPY static /app/static

# DO App Platform will set PORT; we also honor HTTP_PORT for local parity.
ENV RUST_LOG=info,polymarket_btc_5m=debug
EXPOSE 3000

# exec form so PID 1 is the binary (receives SIGTERM directly).
CMD ["/app/bot"]
