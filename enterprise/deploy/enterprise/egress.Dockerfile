FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends squid ca-certificates curl && rm -rf /var/lib/apt/lists/*
USER 13:13
ENTRYPOINT ["squid", "-N", "-f", "/etc/squid/squid.conf"]
