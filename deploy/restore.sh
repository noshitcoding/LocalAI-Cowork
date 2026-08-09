#!/bin/sh
set -eu

deploy_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
backup_dir=${1:-}
confirmation=${2:-}
restore_config=${3:-}
if [ -z "$backup_dir" ] || [ "$confirmation" != '--confirm' ]; then
  echo "Usage: $0 /absolute/backup/directory --confirm [--restore-config]" >&2
  exit 2
fi
case "$backup_dir" in /*) ;; *) echo 'Backup path must be absolute' >&2; exit 2 ;; esac
backup_dir=$(CDPATH= cd -- "$backup_dir" && pwd -P)
case "$backup_dir" in /|"$deploy_dir") echo "Refusing broad restore source: $backup_dir" >&2; exit 2 ;; esac
for required in manifest.txt SHA256SUMS postgres.dump object-store config; do
  [ -e "$backup_dir/$required" ] || { echo "Backup is missing $required" >&2; exit 3; }
done
(cd "$backup_dir" && sha256sum -c SHA256SUMS)
grep -qx 'format=open-cowork-consistent-backup-v1' "$backup_dir/manifest.txt" || { echo 'Unsupported backup format' >&2; exit 3; }

cd "$deploy_dir"
if [ "$restore_config" = '--restore-config' ]; then
  cp -- "$backup_dir/config/docker-compose.yml" docker-compose.yml
  cp -- "$backup_dir/config/Caddyfile" Caddyfile
  [ ! -f "$backup_dir/config/.env" ] || cp -- "$backup_dir/config/.env" .env
  if [ -d "$backup_dir/config/secrets" ]; then
    mkdir -p secrets
    cp -R -- "$backup_dir/config/secrets/." secrets/
    chmod -R go-rwx secrets
  fi
fi

backup_version=$(sed -n 's/^application_version=//p' "$backup_dir/manifest.txt")
current_version=$(sed -n 's/^COWORK_VERSION=//p' .env 2>/dev/null | tail -n 1)
if [ "$backup_version" != unknown ] && [ "$backup_version" != "$current_version" ]; then
  echo "Backup requires COWORK_VERSION=$backup_version; current version is $current_version" >&2
  echo 'Use --restore-config or select the exact backup release before restoring.' >&2
  exit 4
fi
docker compose config --quiet

docker compose stop >/dev/null
docker compose up -d postgres >/dev/null
tries=0
until docker compose exec -T postgres pg_isready -U cowork -d postgres >/dev/null 2>&1; do
  tries=$((tries + 1)); [ "$tries" -lt 60 ] || { echo 'PostgreSQL did not become ready' >&2; exit 5; }; sleep 1
done
docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c 'DROP DATABASE IF EXISTS cowork WITH (FORCE)'
docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c 'CREATE DATABASE cowork'
restore_dump=/tmp/open-cowork-restore.dump
docker compose cp "$backup_dir/postgres.dump" "postgres:$restore_dump" >/dev/null
docker compose exec -T postgres pg_restore --exit-on-error -U cowork -d cowork "$restore_dump"
docker compose exec -T postgres rm -f "$restore_dump"

# The target is the named object-data volume mounted only at /data in the
# stopped MinIO service. Clearing it is intentionally gated by --confirm and a
# verified backup above.
docker compose run --rm --no-deps --entrypoint /bin/sh minio -ec \
  'find /data -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +'
docker compose cp "$backup_dir/object-store/." minio:/data >/dev/null
docker compose up -d >/dev/null

tries=0
port=$(sed -n 's/^COWORK_HTTP_PORT=//p' .env 2>/dev/null | tail -n 1)
[ -n "$port" ] || port=8080
until curl --fail --silent "http://127.0.0.1:$port/readyz" >/dev/null; do
  tries=$((tries + 1)); [ "$tries" -lt 120 ] || { echo 'Restored stack did not become ready' >&2; exit 6; }; sleep 1
done
echo "Restore completed from $backup_dir"
