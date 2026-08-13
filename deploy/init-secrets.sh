#!/bin/sh
set -eu

force=false
if [ "${1:-}" = "--force" ]; then
  force=true
elif [ "$#" -gt 0 ]; then
  echo "usage: $0 [--force]" >&2
  exit 2
fi

secret_directory="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/secrets"
mkdir -p "$secret_directory"
umask 077

random_hex() { openssl rand -hex "$1"; }
write_secret() {
  path="$secret_directory/$1"
  if [ -e "$path" ] && [ "$force" != true ]; then
    echo "$path already exists; rerun with --force only when rotating a stopped deployment" >&2
    exit 1
  fi
  printf '%s' "$2" >"$path"
}

postgres_password="$(random_hex 32)"
write_secret bootstrap_token.txt "$(random_hex 32)"
write_secret postgres_password.txt "$postgres_password"
write_secret database_url.txt "postgres://cowork:$postgres_password@postgres:5432/cowork"
write_secret minio_root_user.txt "cowork$(random_hex 8)"
write_secret minio_root_password.txt "$(random_hex 32)"
write_secret runner_signing_key.txt "$(random_hex 32)"
write_secret storage_master_key.txt "$(openssl rand -base64 32 | tr -d '\n')"

echo "Created Open Cowork deployment secrets in $secret_directory"
