#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
target="${TARGET:-$(rustc -vV | awk '/^host:/ {print $2}')}"
version="${VERSION:?Set VERSION to an unpublished release tag or a local test label}"
if [[ "${SKIP_BUILD:-0}" != 1 ]]; then cargo build --release --locked --target "$target"; fi
binary="${BINARY:-target/$target/release/stream-bridge-server}"
[[ -x "$binary" ]] || { echo "Missing binary: $binary" >&2; exit 1; }
# Build once per package so stale assets cannot silently ship.
(cd dashboard && npm ci --no-audit --no-fund && npm run build)
mkdir -p release
tmp="$(mktemp -d)"
trap 'rm -rf -- "$tmp"' EXIT
package="$tmp/stream-bridge"
mkdir -p "$package/bin" "$package/configs" "$package/docs" "$package/clients"
cp "$binary" "$package/bin/"
cp install.sh setup.sh setup-dashboard.sh run.sh install-service.sh README.md README_CN.md RELEASE_NOTES.md LICENSE "$package/"
cp clients/README*.md clients/client.toml "$package/clients/"
cp config*.toml "$package/configs/"
cp -R docs/. "$package/docs/"
mkdir -p "$package/dashboard"
cp -R dashboard/dist "$package/dashboard/dist"
printf '%s\n' "$version" > "$package/.stream-bridge-install"
chmod +x "$package"/*.sh "$package/bin/stream-bridge-server"
tar -czf "release/stream-bridge-server-$target.tar.gz" -C "$tmp" stream-bridge
cp install.sh release/install.sh
(cd release && shasum -a 256 "stream-bridge-server-$target.tar.gz" > SHA256SUMS)
echo "release/stream-bridge-server-$target.tar.gz"
