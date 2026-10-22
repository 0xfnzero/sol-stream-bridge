# v0.2.0

Server binaries and Rust, Go and TypeScript Bot clients are versioned together at **0.2.0**.

## Shared subscriptions and access control

- One upstream Yellowstone Subscribe stream serves multiple users. Identical filters are deduplicated; updates are routed to matching clients with their own filter names.
- Optional per-user `x-token` / `x-api-key` authentication works together with IP allowlists. Empty configured keys disable only key verification; an empty gRPC IP allowlist still denies all clients.
- Per-stream, per-user and global subscription/filter/account limits, subscription-change rate limits, bounded delivery queues and connection metrics. Account updates and unfiltered transaction/account subscriptions are disabled by default.
- Rust, Go and TypeScript packages expose optional bridge API Key configuration and runnable receiver examples. The upstream provider token stays on the server.

## Operator dashboard and forwarding performance

- React dashboard manages users, key rotation, inherited/custom quotas, account permissions, IP policies and active streams. Settings persist across restarts; revision checks reject stale edits.
- Setup prints a short random username and initial password once. First login requires a password change; Argon2id password hashes, bounded login attempts and revocable sessions protect the loopback-only administration API. Remote access uses SSH forwarding.
- Saves preserve inherited limits. Once settings are persisted, unchanged saves avoid file writes, revision changes and subscription disconnects. Forms protect unsaved drafts and offer explicit conflict recovery; polling pauses in hidden tabs and rejects stale responses.
- Forwarding uses immutable routing snapshots, cached filter aliases, batched slow-client cleanup and payload ownership transfer where possible. TypeScript Shred reception uses a bounded ring queue. No timed batching is added.
- Update the locked Tokio runtime to 1.54.1, replacing the yanked 1.54.0 release and its timer regression.
- Linux packages bundle the dashboard assets and setup scripts; no Rust or Node.js runtime is needed for a prebuilt installation.

## Install and upgrade

- Rust: `cargo add stream-bridge-client@0.2.0 --features yellowstone`.
- Go: `go get github.com/0xfnzero/stream-bridge/clients/go@v0.2.0` (annotated tag `clients/go/v0.2.0`). The original Go module path is retained for compatibility.
- TypeScript: `npm install stream-bridge-client@0.2.0`; receiver CLI: `npx stream-bridge-client@0.2.0 /path/to/client.toml`.
- Server: download this release's `install.sh`, then run `VERSION=v0.2.0 ROLE=shared-receiver ./install.sh` for a new shared receiver, or retain your existing role/directory when upgrading. Assets cover **Linux x86_64 / arm64 (glibc 2.35+)**, with `SHA256SUMS`; no macOS assets.

Upgrades preserve active TOML configuration, dashboard state/password files, tokens and certificates. Existing deployments stay in passthrough mode unless `[grpc.shared]` is explicitly added. Configure provider limits and Bot IPs before enabling shared mode; for a provider limited to one connection, also set source A's `grpc.max_connections = 1`. Run `./run.sh --check` and restart after upgrading. Run `./setup-dashboard.sh` to initialize dashboard credentials, then enable its configuration. See the [shared-mode guide](docs/SHARED.md) and [dashboard guide](docs/DASHBOARD.md).

Shared mode supports Subscribe/Ping/GetVersion with a fixed commitment; replay, data slices and other RPCs remain unsupported. Slow consumers and upstream failures terminate streams: Bots must reconnect, resubscribe and handle gaps. Typed client protobuf versions differ; see [client compatibility](clients/README.md#backpressure-tls-and-compatibility). Local tests and microbenchmarks do not establish production throughput or end-to-end latency.

## 中文说明

服务端与 Rust、Go、TypeScript Bot 客户端统一发布 **0.2.0**。

- 新增共享 Yellowstone 上游：多个用户共用一个 Subscribe 流，过滤器去重后合并，按匹配标签分流并恢复用户过滤器名称。
- API Key（`x-token` / `x-api-key`）与 IP 白名单可同时验证；Key 全为空仅关闭 Key 检查。支持每流、每用户及全局额度、订阅修改频率限制、连接统计，默认禁止账户更新与全量交易/账户订阅。
- 新增 React 管理面板：用户、密钥、继承/独立额度、账户权限、IP 策略与连接管理，修改持久化且带版本冲突检查。初始化终端仅一次显示短随机账号和初始密码，首次登录强制改密，使用 Argon2id 与可撤销会话；远程管理通过 SSH。
- 优化转发路径：不可变路由快照、过滤器和别名缓存、慢客户端批量清理、单持有者消息转移；TypeScript Shred 队列采用有界环形缓冲，不增加定时攒批等待。
- 优化面板交互：保留继承额度、已持久化的无变化保存不打断订阅、防止旧响应覆盖新数据、后台暂停刷新、草稿保护、冲突恢复与分页。
- Tokio 锁定版本更新为 1.54.1，替换已撤回且存在计时器回归的 1.54.0。
- Linux x86_64 / arm64 安装包已包含面板页面和初始化脚本，运行无需 Rust 或 Node.js；不发布 macOS 安装包。三语言客户端均支持 `api_key`，安装命令见上文。

升级保留现有配置、管理状态、密码文件、令牌与证书，不自动切换共享模式。新共享接收端可执行 `VERSION=v0.2.0 ROLE=shared-receiver ./install.sh`；已有安装沿用原来的角色和目录。启用前按上游套餐设置额度和 Bot 白名单；上游限一条连接时，发送端 A 也设 `grpc.max_connections = 1`。运行 `./run.sh --check` 后重启；管理面板需执行 `./setup-dashboard.sh` 并启用相应配置。详见 [共享订阅说明](docs/SHARED_CN.md)、[管理面板说明](docs/DASHBOARD_CN.md)。

共享模式支持固定 commitment 的 Subscribe/Ping/GetVersion，不支持回放、数据切片及其他 RPC。上游断线或慢客户端会终止订阅，Bot 需重连并处理缺口。客户端 protobuf 版本差异见 [兼容说明](clients/README_CN.md)。本地验证不代表生产吞吐或端到端延迟保证。

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
