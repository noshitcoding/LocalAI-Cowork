#!/bin/sh
set -eu

deploy_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
target=${1:-}
backup_root=${2:-"$deploy_dir/backups"}
case "$target" in ''|*[!0-9A-Za-z.+-]*) echo 'Usage: upgrade.sh VERSION [BACKUP_ROOT]' >&2; exit 2 ;; esac
case "$target" in [0-9]*.[0-9]*.[0-9]*) ;; *) echo 'VERSION must be SemVer without a v prefix' >&2; exit 2 ;; esac
cd "$deploy_dir"
[ -f .env ] || { echo 'deploy/.env is required' >&2; exit 3; }
old_version=$(sed -n 's/^COWORK_VERSION=//p' .env | tail -n 1)
[ -n "$old_version" ] || { echo 'COWORK_VERSION is missing from .env' >&2; exit 3; }
[ "$old_version" != "$target" ] || { echo "Already on $target"; exit 0; }

backup_dir=$("$deploy_dir/backup.sh" "$backup_root" | tail -n 1)
[ -f "$backup_dir/postgres.dump" ] || { echo 'Upgrade backup was not created' >&2; exit 4; }
env_tmp="$deploy_dir/.env.upgrade.$$"
awk -v target="$target" 'BEGIN { changed=0 } /^COWORK_VERSION=/ { print "COWORK_VERSION=" target; changed=1; next } { print } END { if (!changed) print "COWORK_VERSION=" target }' .env >"$env_tmp"
chmod --reference=.env "$env_tmp" 2>/dev/null || chmod 600 "$env_tmp"
mv -- "$env_tmp" .env

rollback() {
  trap - EXIT HUP INT TERM
  echo 'Upgrade failed; restoring the pre-upgrade release and consistent backup.' >&2
  awk -v target="$old_version" 'BEGIN { changed=0 } /^COWORK_VERSION=/ { print "COWORK_VERSION=" target; changed=1; next } { print } END { if (!changed) print "COWORK_VERSION=" target }' .env >"$env_tmp"
  mv -- "$env_tmp" .env
  "$deploy_dir/restore.sh" "$backup_dir" --confirm
}
trap 'rollback' EXIT HUP INT TERM

if [ "${COWORK_UPGRADE_BUILD_FROM_SOURCE:-0}" = 1 ]; then
  docker compose build --pull
else
  docker compose pull
fi
docker compose up -d --remove-orphans >/dev/null
port=$(sed -n 's/^COWORK_HTTP_PORT=//p' .env | tail -n 1); [ -n "$port" ] || port=8080
tries=0
until curl --fail --silent "http://127.0.0.1:$port/readyz" >/dev/null; do
  tries=$((tries + 1))
  if [ "$tries" -ge 120 ]; then rollback; exit 5; fi
  sleep 1
done
trap - EXIT HUP INT TERM
echo "Upgrade $old_version -> $target completed; rollback backup: $backup_dir"
