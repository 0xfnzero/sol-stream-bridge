#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
command -v openssl >/dev/null || { echo "openssl is required" >&2; exit 1; }
if [[ ! -f dashboard/dist/index.html ]]; then
  [[ -f dashboard/package-lock.json ]] || { echo "Dashboard assets missing; reinstall the release package" >&2; exit 1; }
  command -v npm >/dev/null || { echo "Source builds need Node.js >=22.12 and npm" >&2; exit 1; }
  (cd dashboard && npm ci --no-audit --no-fund && npm run build)
fi
# Credentials are plain data, never shell code. Preserve them across reruns.
[[ ! -L .dashboard-initial-credentials ]] || { echo "Refusing symlink .dashboard-initial-credentials" >&2; exit 1; }
umask 077
created_credentials=false
if [[ ! -e .dashboard-initial-credentials && ! -e dashboard-admin.toml ]]; then
  initial_user="${DASHBOARD_INITIAL_USERNAME:-sb_$(openssl rand -hex 3)}"
  [[ "$initial_user" =~ ^[a-zA-Z0-9_.-]{3,64}$ ]] || { echo "Username must be 3..64 ASCII letters/digits/._-" >&2; exit 1; }
  case "$(printf '%s' "$initial_user" | tr '[:upper:]' '[:lower:]')" in admin|administrator|root) echo "Choose a less common username" >&2; exit 1;; esac
  initial_password="$(openssl rand -hex 24)"
  created_credentials=true
  (set -o noclobber; printf '%s\n%s\n' "$initial_user" "$initial_password" > .dashboard-initial-credentials)
fi
if [[ -e .dashboard-initial-credentials ]]; then
  [[ -f .dashboard-initial-credentials && -s .dashboard-initial-credentials ]] || { echo "Invalid .dashboard-initial-credentials" >&2; exit 1; }
  chmod 600 .dashboard-initial-credentials
fi
cat <<'HELP'
Dashboard assets and initial account credentials are ready. Existing credentials are preserved.
Enable this table in the shared RECEIVER configuration (only once):

[grpc.shared.dashboard]
listen = "127.0.0.1:8080"
auth_file = "dashboard-admin.toml"
state_file = "dashboard-state.toml"
assets_dir = "dashboard/dist"

Run ./run.sh --check, then restart ./run.sh (or the systemd service).
Open http://127.0.0.1:8080. If retained, read .dashboard-initial-credentials in your terminal:
  line 1 = username (9 characters by default); line 2 = initial password.
Your first login requires changing the password. Later changes use the key button.
Initialization credentials are ignored once dashboard-admin.toml exists; use your saved password.
For a remote server: ssh -N -L 8080:127.0.0.1:8080 user@RECEIVER_IP
See docs/DASHBOARD.md or docs/DASHBOARD_CN.md for configuration and backups.
HELP

if [[ "$created_credentials" == true ]]; then
  printf '\nInitial dashboard username: %s\nInitial dashboard password: %s\n' "$initial_user" "$initial_password"
  echo "Save these credentials securely. First login requires a password change."
fi
