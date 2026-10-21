#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
[[ "$(uname -s)" == Linux && "$EUID" == 0 ]] || { echo "Run with sudo on a systemd Linux host" >&2; exit 1; }
root="$(pwd -P)"
case "$root" in *'"'*|*'\'*|*'%'*|*$'\n'*) echo "Unsupported characters in installation path" >&2; exit 1;; esac
user="${SERVICE_USER:-${SUDO_USER:-root}}"
id "$user" >/dev/null
# Validate as the eventual service user before changing the service.
runuser -u "$user" -- "$root/run.sh" --check
cat > /etc/systemd/system/stream-bridge-server.service <<UNIT
[Unit]
Description=Solana Shred and Yellowstone gRPC bridge
Wants=network-online.target
After=network-online.target
[Service]
Type=simple
User=$user
WorkingDirectory="$root"
ExecStart="$root/run.sh"
Restart=on-failure
RestartSec=3
LimitNOFILE=65536
NoNewPrivileges=true
[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable --now stream-bridge-server
systemctl --no-pager status stream-bridge-server
