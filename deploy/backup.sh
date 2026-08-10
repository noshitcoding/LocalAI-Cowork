#!/bin/sh
set -eu

deploy_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
repo_dir=$(CDPATH= cd -- "$deploy_dir/.." && pwd -P)
backup_root=${1:-"$deploy_dir/backups"}
case "$backup_root" in /*) ;; *) backup_root="$deploy_dir/$backup_root" ;; esac
mkdir -p -- "$backup_root"
backup_root=$(CDPATH= cd -- "$backup_root" && pwd -P)
case "$backup_root" in /|"$deploy_dir"|"$repo_dir") echo "Refusing broad backup target: $backup_root" >&2; exit 2 ;; esac

cd "$deploy_dir"
docker compose config --quiet
running_services=$(docker compose ps --status running --services)
for required in postgres minio; do
  echo "$running_services" | grep -qx "$required" || { echo "$required must be running" >&2; exit 3; }
done

stamp=$(date -u +%Y%m%dT%H%M%SZ)
backup_dir="$backup_root/$stamp"
suffix=0
while [ -e "$backup_dir" ]; do suffix=$((suffix + 1)); backup_dir="$backup_root/$stamp-$suffix"; done
mkdir -m 700 -- "$backup_dir" "$backup_dir/object-store" "$backup_dir/config"

resume_services=''
for service in gateway api worker runner minio; do
  if echo "$running_services" | grep -qx "$service"; then resume_services="$resume_services $service"; fi
done
resumed=false
resume() {
  if [ "$resumed" = false ] && [ -n "$resume_services" ]; then
    docker compose up -d $resume_services >/dev/null
    resumed=true
  fi
}
trap 'resume' EXIT HUP INT TERM

cp -- docker-compose.yml Caddyfile "$backup_dir/config/"
for overlay in docker-compose.*.yml; do [ -f "$overlay" ] && cp -- "$overlay" "$backup_dir/config/"; done
[ ! -f .env ] || cp -- .env "$backup_dir/config/.env"
[ ! -d secrets ] || cp -R -- secrets "$backup_dir/config/secrets"
docker compose config --images >"$backup_dir/images.txt"

# Stop every component that can mutate PostgreSQL or object storage. Database
# and object-store containers remain available long enough to capture them.
docker compose stop gateway api worker runner >/dev/null
db_dump="/tmp/open-cowork-$stamp.dump"
docker compose exec -T postgres pg_dump -U cowork -d cowork --format=custom --file="$db_dump"
docker compose cp "postgres:$db_dump" "$backup_dir/postgres.dump" >/dev/null
docker compose exec -T postgres rm -f "$db_dump"

docker compose stop minio >/dev/null
docker compose cp minio:/data/. "$backup_dir/object-store" >/dev/null

version=$(sed -n 's/^COWORK_VERSION=//p' .env 2>/dev/null | tail -n 1)
[ -n "$version" ] || version=unknown
cat >"$backup_dir/manifest.txt" <<EOF
format=open-cowork-consistent-backup-v1
created_at=$stamp
application_version=$version
postgres_image=postgres:17-alpine
minio_image=quay.io/minio/minio:RELEASE.2025-04-22T22-12-26Z
maintenance_consistent=true
EOF

(cd "$backup_dir" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum >SHA256SUMS)
chmod -R go-rwx "$backup_dir"
resume
trap - EXIT HUP INT TERM
printf '%s\n' "$backup_dir"
