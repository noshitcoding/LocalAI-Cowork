#!/usr/bin/env bash
# Scoped development proof. No existing runtime, mount, WSL configuration or
# distribution is replaced. Run the test as the supplied unprivileged user.
set -euo pipefail
test "$#" -eq 2 || { echo 'usage: qualify_receiver_volume.sh USER TEST_BINARY' >&2; exit 2; }
test "$(id -u)" -eq 0 || { echo 'volume setup requires root' >&2; exit 2; }
proof_user=$1
test "$(id -u "$proof_user")" -ne 0 || { echo 'test user must be unprivileged' >&2; exit 2; }
test_binary=$(readlink -f -- "$2")
test -f "$test_binary" && test -x "$test_binary"
proof_root=$(mktemp -d /run/cowork-receiver-proof.XXXXXXXX)
case "$proof_root" in /run/cowork-receiver-proof.*) ;; *) exit 2 ;; esac
test "$(dirname -- "$proof_root")" = /run
proof_loop=''
cleanup() {
    if mountpoint -q -- "$proof_root/volume"; then
        umount -- "$proof_root/volume" || return 1
    fi
    if test -n "$proof_loop"; then
        test "$(losetup --noheadings --output BACK-FILE "$proof_loop")" = "$proof_root/payload.ext4" || return 1
        losetup --detach "$proof_loop" || return 1
        test -z "$(losetup --associated "$proof_root/payload.ext4")" || return 1
    fi
    test ! -f "$proof_root/payload.ext4" || rm -- "$proof_root/payload.ext4"
    test ! -d "$proof_root/volume" || rmdir -- "$proof_root/volume"
    rmdir -- "$proof_root"
    echo 'fixture_removed=true; mount_absent=true; loop_detached=true'
}
finish() {
    proof_status=$?
    trap - EXIT
    if ! cleanup; then
        echo "fixture cleanup unresolved: $proof_root $proof_loop" >&2
        exit 1
    fi
    exit "$proof_status"
}
trap finish EXIT
chmod 0711 -- "$proof_root"
mkdir -- "$proof_root/volume"
fallocate -l 16M -- "$proof_root/payload.ext4"
mkfs.ext4 -F -q -N 128 -- "$proof_root/payload.ext4"
proof_loop=$(losetup --find --show -- "$proof_root/payload.ext4")
mount -t ext4 -o nodev,nosuid,noexec -- "$proof_loop" "$proof_root/volume"
chown -- "$proof_user" "$proof_root/volume"
chmod 0700 -- "$proof_root/volume"
echo "fixture=$proof_root; loop=$proof_loop; bytes=16777216; user=$proof_user"
runuser -u "$proof_user" -- env COWORK_RECEIVER_TEST_VOLUME="$proof_root/volume" \
    "$test_binary" --ignored --exact \
    insufficient_reserved_volume_rejects_before_acknowledgement_without_sparse_fallback \
    --nocapture --test-threads=1
