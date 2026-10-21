#!/usr/bin/env bash
set -euo pipefail
REPO="${REPO:-0xfnzero/sol-stream-bridge}"
VERSION="${VERSION:-latest}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/stream-bridge}"
ROLE="${ROLE:-source}"
MARKER=.stream-bridge-install
fail() { echo "Error: $*" >&2; exit 1; }
case "$ROLE" in source|receiver|shred-only|private-source|private-receiver) ;; *) fail "Invalid ROLE: $ROLE";; esac
if [[ -z "${TARGET:-}" ]]; then
  case "$(uname -s)" in Darwin) os=apple-darwin;; Linux) os=unknown-linux-gnu;; *) fail "Supported systems: Linux and macOS";; esac
  case "$(uname -m)" in x86_64|amd64) arch=x86_64;; aarch64|arm64) arch=aarch64;; *) fail "Supported architectures: x86_64 and arm64";; esac
  TARGET="$arch-$os"
fi
case "$TARGET" in x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin) ;; *) fail "Unsupported TARGET: $TARGET";; esac
[[ ! -L "$INSTALL_DIR" ]] || fail "INSTALL_DIR must not be a symlink"
mkdir -p "$(dirname "$INSTALL_DIR")"
parent="$(cd "$(dirname "$INSTALL_DIR")" && pwd -P)"
base="$(basename "$INSTALL_DIR")"
INSTALL_DIR="$parent/$base"
case "$base" in ''|.|..) fail "Unsafe INSTALL_DIR";; esac
[[ "$INSTALL_DIR" != / && "$INSTALL_DIR" != "$HOME" ]] || fail "Unsafe INSTALL_DIR"
if [[ -e "$INSTALL_DIR" ]]; then
  [[ -d "$INSTALL_DIR" && -f "$INSTALL_DIR/$MARKER" ]] || fail "Refusing to replace an unmanaged directory: $INSTALL_DIR"
fi
if [[ -n "${BASE_URL:-}" ]]; then base_url="${BASE_URL%/}"
elif [[ "$VERSION" == latest ]]; then base_url="https://github.com/$REPO/releases/latest/download"
else base_url="https://github.com/$REPO/releases/download/$VERSION"; fi
archive="stream-bridge-server-$TARGET.tar.gz"
tmp="$(mktemp -d "$parent/.stream-bridge-install.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
download() {
  if command -v curl >/dev/null; then curl -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null; then wget -q -O "$2" "$1"
  else fail "Install curl or wget first"; fi
}
echo "Installing $REPO ($VERSION, $TARGET) to $INSTALL_DIR"
download "$base_url/$archive" "$tmp/$archive"
download "$base_url/SHA256SUMS" "$tmp/SHA256SUMS"
expected="$(awk -v name="$archive" '$2 == name { print $1 }' "$tmp/SHA256SUMS")"
[[ "$expected" =~ ^[0-9a-fA-F]{64}$ ]] || fail "Missing or invalid SHA256 checksum"
if command -v sha256sum >/dev/null; then actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null; then actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
else fail "sha256sum or shasum is required"; fi
[[ "$expected" == "$actual" ]] || fail "Checksum mismatch"
# Packages contain only regular files/directories under a single root.
tar -tzf "$tmp/$archive" > "$tmp/entries"
awk '($0 !~ /^stream-bridge\// || $0 ~ /(^|\/)\.\.(\/|$)/) { exit 1 }' "$tmp/entries" || fail "Unsafe archive paths"
tar -tvzf "$tmp/$archive" | awk 'substr($0,1,1)!="-" && substr($0,1,1)!="d" {exit 1}' || fail "Archive links/devices are not allowed"
tar -xzf "$tmp/$archive" -C "$tmp"
package="$tmp/stream-bridge"
[[ -f "$package/$MARKER" && -x "$package/bin/stream-bridge-server" && -x "$package/run.sh" && -x "$package/setup.sh" ]] || fail "Incomplete release package"
# Upgrade only program/templates. Retain user configuration, tokens and certificates.
if [[ -d "$INSTALL_DIR" ]]; then
  for file in "$INSTALL_DIR"/*.toml "$INSTALL_DIR"/.bridge-token; do
    [[ ! -e "$file" ]] || cp -p "$file" "$package/"
  done
  [[ ! -d "$INSTALL_DIR/certs" ]] || cp -Rp "$INSTALL_DIR/certs" "$package/"
fi
# Prepare before touching the old install, so setup failures do not break it.
"$package/setup.sh" "$ROLE"
backup=""
if [[ -d "$INSTALL_DIR" ]]; then
  backup="${INSTALL_DIR}.backup.$(date +%Y%m%d%H%M%S).$$"
  mv "$INSTALL_DIR" "$backup"
fi
if ! mv "$package" "$INSTALL_DIR"; then
  [[ -z "$backup" ]] || mv "$backup" "$INSTALL_DIR"
  fail "Could not install package; previous install restored"
fi
[[ -z "$backup" ]] || echo "Previous installation preserved at: $backup"
echo "Installed. Edit $INSTALL_DIR/config.toml and follow the setup instructions above, then run:"
echo "  cd \"$INSTALL_DIR\""
echo "  ./run.sh --check"
echo "  ./run.sh"
echo "Restart any running service after an upgrade. See README_CN.md / README.md."
