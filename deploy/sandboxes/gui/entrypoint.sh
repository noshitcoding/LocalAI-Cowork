#!/usr/bin/env bash
set -euo pipefail

export HOME="${COWORK_HOME:-/workspace/.home}"
mkdir -p "$HOME/.config/libreoffice/4/user"
if [[ ! -f "$HOME/.config/libreoffice/4/user/registrymodifications.xcu" ]]; then
  cat >"$HOME/.config/libreoffice/4/user/registrymodifications.xcu" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry">
  <item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
</oor:items>
EOF
fi
mkdir -p /tmp/.X11-unix
Xvfb "$DISPLAY" -screen 0 "${COWORK_DESKTOP_SIZE:-1440x900x24}" -nolisten tcp >/tmp/xvfb.log 2>&1 &

# Xvfb creates the Unix socket asynchronously. Starting x11vnc before that
# socket exists makes x11vnc exit permanently, which used to make cold starts
# nondeterministic. Keep startup bounded and fail the container if X never
# becomes ready.
x_socket="/tmp/.X11-unix/X${DISPLAY#:}"
for _ in $(seq 1 100); do
  [[ -S "$x_socket" ]] && break
  sleep 0.1
done
if [[ ! -S "$x_socket" ]]; then
  echo "Xvfb did not create $x_socket" >&2
  exit 1
fi

openbox-session >/tmp/openbox.log 2>&1 &

# This listener is bound to loopback inside the sandbox. The runner is expected
# to bridge frames through its authenticated binary WebSocket; no VNC port is
# published by Compose or by the job container.
(
  while true; do
    x11vnc -display "$DISPLAY" -localhost -no6 -forever -shared -nopw -rfbport 5900 >>/tmp/x11vnc-control.log 2>&1 || true
    sleep 0.25
  done
) &
(
  while true; do
    x11vnc -display "$DISPLAY" -localhost -no6 -forever -shared -viewonly -nopw -rfbport 5901 >>/tmp/x11vnc-view.log 2>&1 || true
    sleep 0.25
  done
) &

exec "$@"
