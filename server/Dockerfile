# syntax=docker/dockerfile:1.7
FROM rust:1.95-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates/cowork-contracts/Cargo.toml crates/cowork-contracts/Cargo.toml
COPY crates/cowork-contracts/src crates/cowork-contracts/src
COPY crates/cowork-runtime/Cargo.toml crates/cowork-runtime/Cargo.toml
COPY crates/cowork-runtime/src crates/cowork-runtime/src
COPY server/cowork-server/Cargo.toml server/cowork-server/Cargo.toml
COPY server/cowork-server/src server/cowork-server/src
COPY server/cowork-server/migrations server/cowork-server/migrations
COPY contracts/generated contracts/generated
COPY server/cowork-runner/Cargo.toml server/cowork-runner/Cargo.toml
COPY server/cowork-runner/src server/cowork-runner/src
COPY agents/cowork-device-agent/Cargo.toml agents/cowork-device-agent/Cargo.toml
COPY agents/cowork-device-agent/src agents/cowork-device-agent/src
COPY agents/cowork-local-daemon/Cargo.toml agents/cowork-local-daemon/Cargo.toml
COPY agents/cowork-local-daemon/src agents/cowork-local-daemon/src

RUN cargo build --locked --release -p cowork-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 cowork \
    && useradd --system --uid 10001 --gid cowork --home-dir /nonexistent --shell /usr/sbin/nologin cowork
COPY --from=builder /workspace/target/release/cowork-server /usr/local/bin/cowork-server
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cowork-server"]
