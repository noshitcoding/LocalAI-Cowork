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
COPY server/cowork-server/build.rs server/cowork-server/build.rs
COPY server/cowork-server/src server/cowork-server/src
COPY server/cowork-server/migrations server/cowork-server/migrations
COPY agents/cowork-device-agent/Cargo.toml agents/cowork-device-agent/Cargo.toml
COPY agents/cowork-device-agent/src agents/cowork-device-agent/src
COPY agents/cowork-local-daemon/Cargo.toml agents/cowork-local-daemon/Cargo.toml
COPY agents/cowork-local-daemon/src agents/cowork-local-daemon/src
RUN --mount=type=cache,id=cowork-cargo-registry-rust195,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cowork-runner-target-rust195-alpine,target=/workspace/target,sharing=locked \
    cargo build --locked --release -j 4 -p cowork-runner \
    && cp /workspace/target/release/cowork-runner /workspace/cowork-runner

FROM docker:28-cli
RUN apk add --no-cache kubectl
RUN addgroup -S -g 10001 cowork && adduser -S -D -H -u 10001 -G cowork cowork
COPY --from=builder /workspace/cowork-runner /usr/local/bin/cowork-runner
EXPOSE 8090
ENTRYPOINT ["/usr/local/bin/cowork-runner"]
