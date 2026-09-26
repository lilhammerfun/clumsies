# syntax=docker/dockerfile:1

FROM rust:1.92-slim-bookworm AS builder

WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY crates/server ./crates/server
COPY crates/clumsiesd ./crates/clumsiesd
# The workspace resolves every member's manifest, so the desktop client's
# manifest has to be present even though this image only builds the Server.
COPY apps/desktop ./apps/desktop
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    cargo build --locked --release -p server --bin clumsies-server \
    && cp /workspace/target/release/clumsies-server /tmp/clumsies-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 clumsies

COPY --from=builder /tmp/clumsies-server /usr/local/bin/clumsies-server

USER clumsies
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/clumsies-server"]
