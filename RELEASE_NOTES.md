# Unreleased / 尚未发布

- React operator dashboard for shared gRPC: live usage, API key lifecycle, user quotas/account permissions, IP policy, stream disconnects and durable access settings. Loopback admin API with a separate token; bundled assets added to future Linux packages. See [dashboard guide](docs/DASHBOARD.md).
- 新增 React 管理面板、独立管理员令牌、用户与白名单实时管理及持久化；详见 [中文指南](docs/DASHBOARD_CN.md)。

- Shared Yellowstone mode merges multiple users into one upstream Subscribe stream and routes labelled updates back to their own filters.
- Optional per-user `x-token` / `x-api-key` authentication combines with IP allowlists. Empty key configuration disables only key checks.
- Per-user and global subscription/filter/account limits, account-update permissions (default off), bounded fanout queues and loopback Prometheus metrics.
- Rust, Go and TypeScript source clients add optional `api_key` support. These changes are not included in published v0.1.1 packages or binaries.
- 新增共享上游、用户 Key/IP 双重校验、累计额度、账户更新权限与连接统计；当前需源码构建。详见 [共享订阅说明](docs/SHARED_CN.md)。

# v0.1.1

Server binaries and Rust, Go and TypeScript Bot clients are versioned together at **0.1.1**.

## Changes

- Trusted-private templates now use plaintext TCP (`grpc.insecure = true`), retaining shared tokens and gRPC client IP allowlists. Private receiver setup creates only a token. Public templates retain TLS; Shred uses raw UDP.
- UDP forwarding no longer waits on congested destinations. Rust and Go avoid copying Shred payloads when the Bot queue is full.
- Stalled reverse-tunnel candidates have a bounded open timeout (`grpc.tunnel_open_timeout_ms`, default 1000 ms) within the total connection deadline. Connections are never replayed after forwarding begins.
- TypeScript validates file and API configuration, rejects unknown keys, handles explicit/default HTTP port 80, and matches equivalent IPv4-mapped IPv6 allowlist entries. Rust/server IP matching is also normalized.
- The Rust receiver example consumes Shred while its initial gRPC connection is pending. All three examples are checked end to end through TLS and plaintext TCP bridges.

## Install and upgrade

- Rust: `cargo add stream-bridge-client@0.1.1 --features yellowstone`
- Go: `go get github.com/0xfnzero/stream-bridge/clients/go@v0.1.1` (tag `clients/go/v0.1.1`).
- TypeScript: `npm install stream-bridge-client@0.1.1`; receiver CLI: `npx stream-bridge-client@0.1.1 /path/to/client.toml`.
- Server: use the installer with `VERSION=v0.1.1` and your existing `ROLE` / `INSTALL_DIR`. Release assets cover **Linux x86_64 / arm64 (glibc 2.35+)** only, with `install.sh` and `SHA256SUMS`. macOS binaries are no longer published.

Upgrades preserve active TOML configuration, tokens and certificates. Existing v0.1.0 private deployments keep TLS until you explicitly set `[grpc] insecure = true` on **both** ends, run `./run.sh --check`, and restart both processes. Use plaintext only on a controlled, trusted network. Upstream Yellowstone TLS requirements are independent and remain in effect.

## 中文说明

服务端与 Rust、Go、TypeScript Bot 客户端统一发布 **0.1.1**。主要更新：

- 可信内网模板默认明文 TCP，只需私网 IP、端口和共享令牌，继续保留 gRPC 客户端 IP 白名单；公网模板保留 TLS，Shred 继续使用 UDP。
- UDP 转发不再等待拥塞目标；Rust / Go 队列满时避免复制 Shred 数据。隧道候选连接增加开启超时，受总连接期限约束。
- 修复 TypeScript 默认 / 显式 HTTP 80 端口、配置校验及 IPv4-mapped IPv6 白名单匹配；Rust / 服务端同样统一 IP 匹配。
- Rust 示例在等待首次 gRPC 连接时也能接收 Shred；三语言示例均验证 TLS 和明文 TCP 通道。

服务端仅提供 Linux x86_64 / arm64（glibc 2.35+）安装包，不再发布 macOS 二进制。安装命令见上文。升级保留现有配置、令牌和证书，不会自动关闭旧部署的 TLS。已有可信内网部署切换时，需在**两端**显式设置 `[grpc] insecure = true`，执行 `./run.sh --check` 后重启。上游 Yellowstone 自身要求 TLS 时仍须使用 HTTPS，与桥接传输模式独立。

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
