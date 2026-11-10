FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates squid squidclient \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/squid /var/log/squid /var/spool/squid \
    && chown -R proxy:proxy /run/squid /var/log/squid /var/spool/squid

COPY deploy/egress-proxy/squid.conf /etc/squid/squid.conf

USER proxy:proxy
EXPOSE 3128
CMD ["squid", "--foreground", "-f", "/etc/squid/squid.conf"]
