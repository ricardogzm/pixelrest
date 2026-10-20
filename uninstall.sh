#!/usr/bin/env bash
# Removes the service and binary.
set -euo pipefail
systemctl --user disable --now pixelrest.service 2>/dev/null || true
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/pixelrest.service" "$HOME/.local/bin/pixelrest"
systemctl --user daemon-reload
echo "Config left in ${XDG_CONFIG_HOME:-$HOME/.config}/pixelrest/"
