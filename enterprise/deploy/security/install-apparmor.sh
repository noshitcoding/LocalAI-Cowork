#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "run this installer as root" >&2
  exit 1
fi
if ! command -v apparmor_parser >/dev/null 2>&1; then
  echo "apparmor_parser is required" >&2
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source_profile="${script_dir}/open-cowork-sandbox.apparmor"
target_profile="/etc/apparmor.d/open-cowork-sandbox"
if [[ ! -f "${source_profile}" ]]; then
  echo "profile source is missing: ${source_profile}" >&2
  exit 1
fi

install -o root -g root -m 0644 "${source_profile}" "${target_profile}"
apparmor_parser -r -W "${target_profile}"
if command -v aa-status >/dev/null 2>&1; then
  aa-status | grep -F 'open-cowork-sandbox' >/dev/null
fi
echo "loaded enforced AppArmor profile open-cowork-sandbox"
