#!/usr/bin/env bash
set -euo pipefail
umask 077
bundle_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
cd -- "$bundle_dir"
sha256sum --strict --status --check SHA256SUMS
if [[ $(uname -m) != x86_64 ]]; then
  printf '%s\n' 'This bundle requires native Linux x86_64.' >&2
  exit 1
fi
export COWORK_DATA_DIR=${COWORK_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/localai-cowork-v2/server-data}
if [[ $COWORK_DATA_DIR != /* ]]; then
  printf '%s\n' 'COWORK_DATA_DIR must be an absolute private directory.' >&2
  exit 1
fi
mkdir -p -m 700 -- "$COWORK_DATA_DIR"
export COWORK_WEB_DIR="$bundle_dir/web"
export COWORK_PORT=${COWORK_PORT:-43821}
if [[ ${COWORK_ENABLE_TEST_PROVIDER:-0} != 0 ]]; then
  printf '%s\n' 'The installation bundle does not enable synthetic test providers.' >&2
  exit 1
fi
exec "$bundle_dir/bin/cowork-daemon"
