# Unreleased (main)

- Trusted-private templates now use plaintext TCP (`grpc.insecure = true`) and retain shared tokens and client IP allowlists. Private receiver setup creates only a token. Public templates continue to use TLS; Shred continues to use UDP.
- Upgrades preserve active configuration and credentials. Existing installations must explicitly change `insecure` on both ends to switch transport. Published v0.1.0 private templates still use TLS.
- CI exercises all three Bot clients through both TLS and plaintext TCP bridges.

# v0.1.0

Initial release: concurrent Shred UDP forwarding and Yellowstone reverse tunnels, TLS/token authentication, receiver IP allowlists, private-network templates, and Rust/Go/TypeScript Bot clients.

Includes four-platform binary packaging, checksum-verified installation, setup/run scripts, optional systemd integration, and local end-to-end tests.

## Packages

- Rust / crates.io: `stream-bridge-client` 0.1.0 (enable `yellowstone` for typed gRPC Bot APIs).
- Go: `github.com/0xfnzero/stream-bridge/clients/go`, version tag `clients/go/v0.1.0`.
- TypeScript / npm: `stream-bridge-client` 0.1.0, including declarations and the receiver CLI.
- Binary installers: four Linux/macOS, x86_64/arm64 packages in the GitHub release, plus `install.sh` and `SHA256SUMS`.

The SDK packages are intended for Bot integration and expose data reception APIs. They are separate from installing a running bridge service.

```toml
# Rust, Cargo.toml
stream-bridge-client = { version = "0.1.0", features = ["yellowstone"] }
```

```bash
# Go
go get github.com/0xfnzero/stream-bridge/clients/go@v0.1.0
# TypeScript
npm install stream-bridge-client
```

## Compatibility

The bridge is protocol-transparent. Typed clients expose fields supported by their pinned protobuf dependencies. Shred is delivered as raw bytes; transaction reconstruction is the Bot's responsibility. Empty receiver gRPC allowlists deny all clients. UDP-only operation does not require TLS credentials.
