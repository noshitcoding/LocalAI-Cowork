# syntax=docker/dockerfile:1.7
FROM rust:1.95-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY crates/cowork-contracts/Cargo.toml crates/cowork-contracts/Cargo.toml
COPY crates/cowork-contracts/src crates/cowork-contracts/src
COPY crates/cowork-runtime/Cargo.toml crates/cowork-runtime/Cargo.toml
COPY crates/cowork-runtime/src crates/cowork-runtime/src
COPY server/cowork-runner/Cargo.toml server/cowork-runner/Cargo.toml
COPY server/cowork-runner/src server/cowork-runner/src
COPY server/cowork-server/Cargo.toml server/cowork-server/Cargo.toml
COPY server/cowork-server/src server/cowork-server/src
COPY server/cowork-server/migrations server/cowork-server/migrations
COPY agents/cowork-device-agent/Cargo.toml agents/cowork-device-agent/Cargo.toml
COPY agents/cowork-device-agent/src agents/cowork-device-agent/src
COPY agents/cowork-local-daemon/Cargo.toml agents/cowork-local-daemon/Cargo.toml
COPY agents/cowork-local-daemon/src agents/cowork-local-daemon/src
RUN cargo build --locked --release -p cowork-runner

FROM docker:28-cli
RUN addgroup -S -g 10001 cowork && adduser -S -D -H -u 10001 -G cowork cowork
COPY --from=builder /workspace/target/release/cowork-runner /usr/local/bin/cowork-runner
EXPOSE 8090
ENTRYPOINT ["/usr/local/bin/cowork-runner"]
