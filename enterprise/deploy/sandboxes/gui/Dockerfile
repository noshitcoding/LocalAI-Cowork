ARG PLAYWRIGHT_IMAGE=mcr.microsoft.com/playwright:v1.62.1-noble
FROM ${PLAYWRIGHT_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive \
    DISPLAY=:99 \
    HOME=/home/cowork

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        dbus-x11 jq libreoffice openbox poppler-utils python3-venv socat tini x11vnc xvfb \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 cowork \
    && useradd --system --uid 10001 --gid cowork --create-home --home-dir /home/cowork cowork \
    && mkdir -p /workspace /artifacts \
    && chown -R cowork:cowork /workspace /artifacts /home/cowork

RUN mkdir -p /opt/cowork \
    && npm install --prefix /opt/cowork --omit=dev playwright@1.62.1 \
    && npm cache clean --force \
    && python3 -m venv /opt/cowork/office-venv \
    && /opt/cowork/office-venv/bin/pip install --no-cache-dir \
        openpyxl==3.1.5 python-docx==1.2.0 python-pptx==1.0.2 pypdf==6.0.0

COPY deploy/sandboxes/gui/entrypoint.sh /usr/local/bin/cowork-gui-entrypoint
COPY deploy/sandboxes/gui/browser-tool.mjs /opt/cowork/browser-tool.mjs
COPY deploy/sandboxes/gui/office-tool.py /opt/cowork/office-tool.py
RUN chmod 0755 /usr/local/bin/cowork-gui-entrypoint

USER 10001:10001
WORKDIR /workspace
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/cowork-gui-entrypoint"]
