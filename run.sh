#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
config="${CONFIG:-config.toml}"
if [[ "${1:-}" == *.toml ]]; then config="$1"; shift; fi
# A token file is data, never sourced as shell code. An exported token takes precedence.
if [[ -z "${BRIDGE_TOKEN:-}" && -f .bridge-token ]]; then
  BRIDGE_TOKEN="$(cat .bridge-token)"
  export BRIDGE_TOKEN
fi
if [[ -f .dashboard-initial-credentials ]]; then
  if [[ -z "${DASHBOARD_INITIAL_USERNAME:-}" ]]; then
    IFS= read -r DASHBOARD_INITIAL_USERNAME < .dashboard-initial-credentials
    export DASHBOARD_INITIAL_USERNAME
  fi
  if [[ -z "${DASHBOARD_INITIAL_PASSWORD:-}" ]]; then
    DASHBOARD_INITIAL_PASSWORD="$(sed -n '2p' .dashboard-initial-credentials)"
    export DASHBOARD_INITIAL_PASSWORD
  fi
fi
binary=bin/stream-bridge-server
if [[ ! -x "$binary" ]]; then
  [[ -f Cargo.toml ]] || { echo "Missing binary; reinstall the release package" >&2; exit 1; }
  cargo build --release --locked
  binary=target/release/stream-bridge-server
fi
exec "$binary" --config "$config" "$@"
