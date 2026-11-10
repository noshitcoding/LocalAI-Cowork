#!/bin/sh
set -eu
export MINIO_ROOT_USER="$(cat "$MINIO_ROOT_USER_FILE")"
export MINIO_ROOT_PASSWORD="$(cat "$MINIO_ROOT_PASSWORD_FILE")"
exec minio "$@"
