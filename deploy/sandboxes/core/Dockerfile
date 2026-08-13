FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git jq openssh-client python3 ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 cowork \
    && useradd --system --uid 10001 --gid cowork --create-home --home-dir /workspace cowork

COPY deploy/sandboxes/core/mcp-tool.py /opt/cowork/mcp-tool.py

USER 10001:10001
WORKDIR /workspace
CMD ["/bin/bash"]
