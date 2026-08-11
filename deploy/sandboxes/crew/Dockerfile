FROM python:3.12.11-slim-bookworm

ENV CREWAI_TRACING_ENABLED=false \
    HOME=/tmp/cowork-home \
    LITELLM_LOCAL_MODEL_COST_MAP=True \
    OTEL_SDK_DISABLED=true \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    XDG_CACHE_HOME=/tmp/cowork-cache \
    XDG_CONFIG_HOME=/tmp/cowork-config \
    XDG_DATA_HOME=/tmp/cowork-data

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git jq ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 cowork \
    && useradd --system --uid 10001 --gid cowork --create-home --home-dir /home/cowork cowork \
    && mkdir -p /opt/cowork/crew-runtime /workspace \
    && chown -R cowork:cowork /home/cowork /workspace

COPY app/src-tauri/python/crew_runtime/requirements.txt /opt/cowork/crew-runtime/requirements.txt
RUN python3 -m pip install --no-cache-dir --requirement /opt/cowork/crew-runtime/requirements.txt

COPY app/src-tauri/python/crew_runtime/main.py /opt/cowork/crew-runtime/main.py
COPY app/src-tauri/python/crew_runtime/crew_tools.py /opt/cowork/crew-runtime/crew_tools.py
RUN python3 /opt/cowork/crew-runtime/main.py status > /tmp/crew-runtime-status.json \
    && python3 -c "import json; status=json.load(open('/tmp/crew-runtime-status.json', encoding='utf-8')); assert status['runtimeCompatible'] and status['crewaiVersion'] == '1.15.8' and status['runtimeSchemaVersion'] == 2, status" \
    && rm /tmp/crew-runtime-status.json

USER 10001:10001
WORKDIR /workspace
CMD ["python3", "/opt/cowork/crew-runtime/main.py", "status"]
