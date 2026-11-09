FROM rust:1.94.1-bookworm@sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55
RUN apt-get update && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add rustfmt clippy \
    && useradd --create-home --uid 10001 qualification
ENV CARGO_HOME=/home/qualification/.cargo CARGO_TARGET_DIR=/home/qualification/target
COPY --chown=10001:10001 . /workspace
USER 10001:10001
WORKDIR /workspace
# Fetch dependencies during image construction. The actual qualification has no network.
RUN cargo fetch --locked
CMD ["python3", "tools/qualification/run_linux.py"]
