# Build this repository's actual daemon and frontend together. Operators can pin
# these base image arguments to registry digests for their own release process.
ARG RUST_IMAGE=rust:1.94-bookworm
ARG NODE_IMAGE=node:24-bookworm
ARG DOCKER_CLI_IMAGE=docker:28-cli
FROM ${DOCKER_CLI_IMAGE} AS docker-client
FROM ${NODE_IMAGE} AS frontend
WORKDIR /build
COPY package.json package-lock.json ./
COPY apps/web apps/web
COPY packages/client packages/client
COPY packages/ui packages/ui
COPY tsconfig.json ./
RUN npm ci --ignore-scripts && npm run build

FROM ${RUST_IMAGE} AS backend
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY services services
COPY tools tools
COPY deploy/v2/android-private-crew.py deploy/v2/android-private-crew.py
RUN cargo build --release --locked --jobs 2 -p cowork-daemon

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 curl python3 python3-venv python3-docx python3-openpyxl python3-pypdf python3-reportlab \
    chromium libreoffice-writer libreoffice-calc libreoffice-impress \
    git gh tini fonts-dejavu-core fonts-noto-core && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 1000 cowork && useradd -u 1000 -g cowork -m cowork
ARG PLAYWRIGHT_VERSION=1.58.0
ARG PYTHON_PPTX_VERSION=1.0.2
# Copies preserve the venv prefix when executable discovery canonicalizes its path.
RUN python3 -m venv --copies --system-site-packages /opt/cowork/python \
    && /opt/cowork/python/bin/pip install --no-cache-dir "playwright==${PLAYWRIGHT_VERSION}" "python-pptx==${PYTHON_PPTX_VERSION}"
# The desktop's document lock (python-docx, pypdfium2, ...) shadows the older Debian packages.
COPY tools/toolbox/requirements-documents.lock tools/toolbox/runtime_install.py /opt/cowork/setup/
RUN /opt/cowork/python/bin/pip install --no-cache-dir --require-hashes --only-binary=:all: --no-deps \
    -r /opt/cowork/setup/requirements-documents.lock
# Pinned Tectonic plus its offline LaTeX cache, built once here (needs network) and only read at
# run time. The warm marker binds the cache to this exact binary; the daemon finds it on PATH.
RUN /usr/bin/python3 -I /opt/cowork/setup/runtime_install.py --data-dir /opt/cowork/document-runtime --tectonic-only \
    && ln -s "$(/usr/bin/python3 -I -c 'import json;print(json.load(open("/opt/cowork/document-runtime/toolbox.json"))["programs"]["tectonic"])')" /usr/local/bin/tectonic \
    && rm -rf /opt/cowork/document-runtime/tool-runtime/tmp /opt/cowork/document-runtime/tool-runtime/home \
    && chmod -R a+rX,go-w /opt/cowork/document-runtime
ENV PATH=/opt/cowork/python/bin:$PATH \
    COWORK_DOCUMENT_RUNTIME_DIR=/opt/cowork/document-runtime/tool-runtime
COPY --from=docker-client /usr/local/bin/docker /usr/local/bin/docker
COPY --from=backend /build/target/release/cowork-daemon /usr/local/bin/cowork-daemon
COPY --from=frontend /build/apps/web/dist /opt/cowork/web
COPY deploy/v2/entrypoint.py /opt/cowork/entrypoint.py
COPY --from=backend /build/Cargo.lock /opt/cowork/release-sources/Cargo.lock
COPY --from=frontend /build/package-lock.json /build/package.json /opt/cowork/release-sources/
COPY --from=frontend /build/apps/web/package.json /opt/cowork/release-sources/apps/web/package.json
COPY --from=frontend /build/packages/client/package.json /opt/cowork/release-sources/packages/client/package.json
COPY --from=frontend /build/packages/ui/package.json /opt/cowork/release-sources/packages/ui/package.json
COPY tools/release/sbom.py tools/release/runtime_inventory.py tools/release/container_inventory.py /opt/cowork/release-tools/
# Only the fresh image build contains no operator keys, mounted data or sessions.
# The bill records actual installed OS packages, native imports and private wheels.
RUN COWORK_CONTAINER_BUILD_INVENTORY=1 /usr/bin/python3 /opt/cowork/release-tools/container_inventory.py \
    --source-root /opt/cowork/release-sources --output /opt/cowork/sbom.spdx.json
ENV COWORK_DATA_DIR=/var/lib/cowork COWORK_WEB_DIR=/opt/cowork/web \
    COWORK_PORT=43821 COWORK_BIND_ADDRESS=0.0.0.0 PYTHONUNBUFFERED=1
USER 1000:1000
EXPOSE 43821
STOPSIGNAL SIGINT
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s \
    CMD curl --fail --silent http://127.0.0.1:43821/health >/dev/null || exit 1
ENTRYPOINT ["/usr/bin/tini","--","/usr/bin/python3","/opt/cowork/entrypoint.py"]
