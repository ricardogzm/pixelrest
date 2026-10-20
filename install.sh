#!/usr/bin/env bash
# Builds and installs pixelrest as a systemd user service.
set -euo pipefail
cd "$(dirname "$0")"

BIN="$HOME/.local/bin/pixelrest"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

cargo build --release

systemctl --user stop pixelrest.service 2>/dev/null || true

mkdir -p "$(dirname "$BIN")" "$UNIT_DIR"

install -m755 target/release/pixelrest "$BIN"
install -m644 packaging/pixelrest.service "$UNIT_DIR/pixelrest.service"
systemctl --user daemon-reload
systemctl --user enable --now pixelrest.service
sleep 1
systemctl --user --no-pager --lines=0 status pixelrest.service | head -3
echo
"$BIN" status
