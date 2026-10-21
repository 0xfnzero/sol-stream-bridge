<div align="center">
  <h1>Stream Bridge</h1>
  <p>Solana Shred and Yellowstone gRPC forwarding with Rust, Go and TypeScript bot clients</p>
  <a href="https://github.com/0xfnzero/stream-bridge/releases"><img src="https://img.shields.io/github/v/release/0xfnzero/stream-bridge" alt="GitHub Release"></a>
  <a href="https://crates.io/crates/stream-bridge-client"><img src="https://img.shields.io/crates/v/stream-bridge-client.svg" alt="Rust client"></a>
  <a href="https://www.npmjs.com/package/stream-bridge-client"><img src="https://img.shields.io/npm/v/stream-bridge-client.svg" alt="TypeScript client"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
</div>

<p align="center">
  <a href="README.md">English</a> · <a href="README_CN.md">中文</a> ·
  <a href="clients/README.md">Bot clients</a> ·
  <a href="docs/DEPLOYMENT.md">Deployment</a> ·
  <a href="RELEASE_NOTES.md">Releases</a>
</p>

Forward your machine's raw Solana Shred, Jito proxy UDP output and Yellowstone gRPC to another server. Run UDP and gRPC together or separately, over public or reachable private networks. Bots consume raw packets and decoded Yellowstone updates through dedicated clients.

**Server:** `stream-bridge-server` · **Bot SDK (Rust/npm):** `stream-bridge-client` · **Go module:** `github.com/0xfnzero/stream-bridge/clients/go`

Source version: **0.1.0**. See [release notes](RELEASE_NOTES.md) for package publication status.

## Contents

- [Features](#features)
- [Script installation](#installation)
- [Configuration and startup](#quick-start)
- [Private networks](#private-network)
- [Bot clients](#clients)
- [Runtime behavior](#behavior)
- [Development](#development)
- [Related projects](#related)

<a id="features"></a>
## Features

1. Multiple UDP listeners and destinations; forwards original Shred payloads without changing datagram boundaries.
2. Accepts Jito proxy UDP output and raw Shred pushed directly to an IP and port.
3. Reverse TCP tunnels preserve Yellowstone HTTP/2, bidirectional subscriptions, filter updates, ping, metadata and x-token.
4. Receiver-side gRPC IP allowlists reject unauthorized clients before opening an upstream tunnel. Empty lists deny everyone.
5. TLS and shared-token authentication protect the reverse tunnel. The source's Yellowstone port need not be public.
6. Private-IP destinations and private-interface listeners for reachable VPC networks.
7. Rust and Go channels / TypeScript async iterators deliver raw Shred; all three expose typed Yellowstone updates.
8. Prebuilt installers, checksum verification, configuration-preserving upgrades, optional systemd service, counters and connection limits.

<a id="installation"></a>
## Script installation

Linux (glibc 2.35+) and macOS, x86_64 / arm64. The installer selects a prebuilt binary; users do not need Rust. Alpine/musl is not covered by these binaries.

On receiver B:

```bash
curl -fsSL -o install.sh https://github.com/0xfnzero/stream-bridge/releases/latest/download/install.sh
chmod +x install.sh
ROLE=receiver ./install.sh
cd "$HOME/stream-bridge"
```

Download the same installer on source A, then:

```bash
ROLE=source ./install.sh
cd "$HOME/stream-bridge"
```

Roles: `source`, `receiver`, `shred-only`, `private-source`, `private-receiver`. Receiver setup requires `openssl` and creates a token plus a self-signed TLS certificate. Configure addresses and share credentials before starting.

Pin a version or directory:

```bash
VERSION=v0.1.0 ROLE=source INSTALL_DIR=/opt/stream-bridge ./install.sh
```

The directory must be writable. Unmanaged directories are never replaced. Upgrades preserve root TOML files, `.bridge-token` and `certs/`; the previous installation is retained as `.backup.*`. Restart the process after upgrading. Backups contain credentials and must be protected accordingly.

<a id="quick-start"></a>
## Configuration and startup

### Receiver B

Edit `config.toml`. The generated receiver template listens for tunnels on `0.0.0.0:19000`, serves local bots on `127.0.0.1:10000`, and permits `127.0.0.1` in `grpc.allowed_sources`.

For remote bots, bind `grpc.listen` to the intended interface and add their actual source IPs. **An omitted or empty gRPC allowlist denies all clients.**

Securely copy `.bridge-token` and `certs/tunnel-cert.pem` to the same paths on source A. Keep `tunnel-key.pem` on B. `run.sh` reads the token as data, never as shell code; an exported `BRIDGE_TOKEN` takes precedence.

### Source A

Edit `config.toml`: replace `203.0.113.10` with B's public or private IP, update raw Shred provider IPs, and set `grpc.upstream` to your local Yellowstone `host:port`.

The tunnel's `server_name` must match its certificate SAN. Receiver setup uses `bridge.example.com` by default; DNS resolution is not required because `tunnel_address` supplies the IP. Use `TLS_NAME=your.domain ROLE=receiver ./install.sh` to generate a different certificate name.

The default Jito route receives UDP on `127.0.0.1:20000`; set your proxy's `--dest-ip-ports` accordingly. Raw Shred arrives on `0.0.0.0:20001`. B must run a UDP consumer or forwarding route on the configured destination ports.

### Start

Start B first, then A:

```bash
./run.sh --check
./run.sh
```

For an alternate configuration: `./run.sh configs/config.shred-only.toml` after editing its destinations. UDP-only mode needs no tunnel token or certificate. Relative paths resolve from the installation directory. Restart to apply changes.

Optional Linux systemd service:

```bash
sudo ./install-service.sh
sudo systemctl restart stream-bridge-server
sudo journalctl -u stream-bridge-server -f
```

The service runs as the invoking sudo user and reads `.bridge-token`. Foreground `./run.sh` works on both Linux and macOS; stop it with Ctrl+C.

<a id="private-network"></a>
## Private networks

Install A/B with `ROLE=private-source` and `ROLE=private-receiver`. Replace example addresses `10.0.0.10`, `10.0.0.20` and client `10.0.0.30`. The private receiver forwards UDP `20000/20001` to local bot ports `21000/21001`; gRPC clients use B's private `10000/TCP`.

Being in the same region does not guarantee connectivity: use a reachable VPC, peering or VPN. The bridge uses system routing, does not create cloud networks, and does not silently fall back to a public IP. TLS, tunnel tokens and client allowlists remain enabled.

<a id="clients"></a>
## Bot clients

See [client integration](clients/README.md) for package installation, API examples and lifecycle handling. All three standalone examples use `clients/client.toml`:

```bash
cargo run --locked --example receive --features yellowstone -- clients/client.toml
(cd clients/go && go run ./examples/receive ../client.toml)
(cd clients/typescript && npm ci && npm run receive -- ../client.toml)
```

Rust can embed the receiver inside a Bot process. Go and TypeScript clients connect to the running Rust receiver. Clients deliver raw Shred bytes with source/timestamp and typed Yellowstone transaction, account, slot and other supported updates. Package-manager availability is tracked in [release notes](RELEASE_NOTES.md).

<a id="behavior"></a>
## Runtime behavior

- UDP is best effort. No Shred reconstruction, FEC, deduplication or transaction decoding is performed. Full Bot queues drop new packets and count them.
- The upstream Jito proxy handles its own authentication and subscription. This tool consumes available UDP output; it cannot restore discontinued upstream services.
- gRPC data follows client subscriptions. Idle tunnels reconnect automatically; interrupted subscriptions must be reopened by the Bot. No historical replay is performed.
- `BRIDGE_TOKEN` authenticates the bridge tunnel, not Yellowstone clients. Client-to-receiver encryption depends on the upstream Yellowstone service's own TLS configuration.
- Allowlists check the socket peer IP. Behind NAT, use the translated address; through a proxy, only the proxy IP is visible.
- See [deployment details](docs/DEPLOYMENT.md) for ports, TLS, queues and operating limits.

<a id="development"></a>
## Development

- `src/`: Rust bridge and Bot client APIs.
- `clients/go/`, `clients/typescript/`: clients and runnable examples.
- `examples/`: Rust receiver and embedded-Bot examples.
- `install.sh`, `setup.sh`, `run.sh`: installation, initialization, startup.
- `.github/workflows/`: tests, four-platform releases, client publication.

```bash
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
(cd clients/go && go test -race ./...)
(cd clients/typescript && npm ci && npm test)
python3 tools/smoke_clients.py
python3 -m unittest discover -s tests -p 'test_install.py'
```

The smoke test uses a local mock Yellowstone and real source/receiver processes. It checks Shred, slot and transaction delivery plus shutdown for all three examples. Configure a live upstream separately for production verification.

<a id="related"></a>
## Related projects

- [sol-trade-sdk](https://github.com/0xfnzero/sol-trade-sdk): Solana DEX transaction construction and submission.
- [sol-parser-sdk](https://github.com/0xfnzero/sol-parser-sdk): transaction and account event parsing.
- [sol-shred-sdk](https://github.com/0xfnzero/sol-shred-sdk): raw Shred decoding.
- [grpc-benchmark](https://github.com/0xfnzero/grpc-benchmark): gRPC and Shred latency measurement.
