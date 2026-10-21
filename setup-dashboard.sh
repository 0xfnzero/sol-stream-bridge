#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
command -v openssl >/dev/null || { echo "openssl is required" >&2; exit 1; }
# Never overwrite a token or follow a symlink supplied as a token file.
[[ ! -L .dashboard-token ]] || { echo "Refusing symlink .dashboard-token" >&2; exit 1; }
umask 077
if [[ ! -e .dashboard-token ]]; then
  (set -o noclobber; openssl rand -hex 32 > .dashboard-token)
fi
[[ -f .dashboard-token && -s .dashboard-token ]] || { echo "Invalid .dashboard-token" >&2; exit 1; }
chmod 600 .dashboard-token
if [[ ! -f dashboard/dist/index.html ]]; then
  [[ -f dashboard/package-lock.json ]] || { echo "Dashboard assets missing; reinstall the release package" >&2; exit 1; }
  command -v npm >/dev/null || { echo "Source builds need Node.js >=22.12 and npm" >&2; exit 1; }
  (cd dashboard && npm ci --no-audit --no-fund && npm run build)
fi
cat <<'HELP'
Dashboard assets and administrator token are ready. Existing keys are preserved.
Enable this table in the shared RECEIVER configuration (only once):

[grpc.shared.dashboard]
listen = "127.0.0.1:8080"
token_env = "DASHBOARD_TOKEN"
state_file = "dashboard-state.toml"
assets_dir = "dashboard/dist"

Run ./run.sh --check, then restart ./run.sh (or the systemd service).
Open http://127.0.0.1:8080 and log in with the contents of .dashboard-token.
For a remote server: ssh -N -L 8080:127.0.0.1:8080 user@RECEIVER_IP
See docs/DASHBOARD.md or docs/DASHBOARD_CN.md for configuration and backups.
HELP
