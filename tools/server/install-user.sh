#!/usr/bin/env bash
set -euo pipefail
umask 077
source_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
cd -- "$source_dir"
sha256sum --strict --status --check SHA256SUMS
destination=${1:-${XDG_DATA_HOME:-$HOME/.local/share}/localai-cowork-v2/server-bundle}
if [[ $destination != /* || -e $destination || -L $destination ]]; then
  printf '%s\n' 'Supply a new absolute installation directory; an existing installation is never replaced.' >&2
  exit 1
fi
mkdir -p -m 700 -- "$(dirname -- "$destination")"
mkdir -m 700 -- "$destination"
cp -R -- "$source_dir/." "$destination/"
chmod 700 -- "$destination/start.sh" "$destination/install-user.sh" "$destination/bin/cowork-daemon"
printf 'Prepared personal V2 server files in %s\n' "$destination"
printf '%s\n' 'No service was started. Run start.sh, or adapt the included user systemd service to this absolute path.'
