<div align="center">
  <h1>Stream Bridge</h1>
  <p>Solana Shred 与 Yellowstone gRPC 转发桥接，支持 Rust、Go、TypeScript Bot</p>
  <a href="https://github.com/0xfnzero/stream-bridge/releases"><img src="https://img.shields.io/github/v/release/0xfnzero/stream-bridge" alt="GitHub Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
</div>

<p align="center">
  <a href="README.md">English</a> · <a href="README_CN.md">中文</a> ·
  <a href="clients/README_CN.md">Bot 客户端</a> ·
  <a href="docs/DEPLOYMENT_CN.md">部署说明</a> ·
  <a href="RELEASE_NOTES.md">发布说明</a>
</p>

将自己机器上的原始 Shred、Jito proxy UDP 输出和 Yellowstone gRPC 提供给另一台服务器。一个进程支持同时转发 Shred 和 gRPC，也可以分别启用；可通过公网或互通私网连接，Bot 通过客户端直接读取数据。

**服务端：** `stream-bridge-server` · **Bot 客户端（Rust/npm）：** `stream-bridge-client` · **Go：** `github.com/0xfnzero/stream-bridge/clients/go`

当前源码版本：**0.1.0**。包发布状态见 [发布说明](RELEASE_NOTES.md)。

## 目录

- [项目特性](#features)
- [脚本安装](#installation)
- [配置与启动](#quick-start)
- [内网转发](#private-network)
- [Bot 客户端](#clients)
- [运行边界](#behavior)
- [项目结构与验证](#development)
- [相关项目](#related)

<a id="features"></a>
## 项目特性

1. **Shred 多路转发**：多个 UDP 监听、多目标复制，保留每个数据包的原始字节。
2. **Jito / 原始 UDP**：接入 Jito proxy 的 UDP 输出，也可接收提供方推送到 IP+端口的原始 Shred。
3. **Yellowstone 兼容**：反向 TCP 隧道透传 HTTP/2、双向 Subscribe、过滤器、ping、metadata 和 `x-token`。
4. **gRPC IP 白名单**：在接收端分配隧道前拒绝未授权客户端；空白名单默认拒绝所有客户端。
5. **TLS 与令牌认证**：保护发送端到接收端的反向隧道；发送端无需为 Yellowstone 开公网入口。
6. **私网通道**：直接使用 VPC 私网 IP，接收端可仅绑定私网接口。
7. **三语言 Bot 接入**：Rust/Go channel、TypeScript async iterator 接收 Shred；三语言均交付解码后的 Yellowstone 消息。
8. **脚本运维**：预编译安装包、SHA256 校验、配置保留升级、systemd 启动、统计日志和连接限制。

<a id="installation"></a>
## 脚本安装

Linux（glibc 2.35+）和 macOS，支持 x86_64 / arm64。安装脚本自动选择平台，不需要自行安装 Rust 或编译。Linux musl/Alpine 不包含在预编译包中。

```bash
curl -fsSL -o install.sh https://github.com/0xfnzero/stream-bridge/releases/latest/download/install.sh
chmod +x install.sh
# 目标服务器 B：生成 receiver 配置、令牌和自签 TLS 证书
ROLE=receiver ./install.sh
cd "$HOME/stream-bridge"
```

在发送服务器 A 执行相同下载步骤，然后：

```bash
ROLE=source ./install.sh
cd "$HOME/stream-bridge"
```

可选角色：`source`、`receiver`、`shred-only`、`private-source`、`private-receiver`。接收端初始化需要 `openssl`。脚本只安装并准备文件，完成地址和凭据配置后再启动。

指定版本或目录：

```bash
VERSION=v0.1.0 ROLE=source INSTALL_DIR=/opt/stream-bridge ./install.sh
```

目录需对当前用户可写。安装脚本拒绝覆盖非本工具管理的目录；升级保留根目录 TOML、`.bridge-token` 和 `certs/`，旧安装保留为 `.backup.*`，便于回滚。升级后需重启进程。备份包含凭据，应与安装目录一样保管。

<a id="quick-start"></a>
## 配置与启动

### 1. 接收端 B

安装后编辑 `config.toml`，主要字段如下（其余 TLS 等配置保留模板默认值）：

```toml
role = "receiver"

[grpc]
tunnel_address = "0.0.0.0:19000"
listen = "127.0.0.1:10000"
allowed_sources = ["127.0.0.1"]
token_env = "BRIDGE_TOKEN"
cert_file = "certs/tunnel-cert.pem"
key_file = "certs/tunnel-key.pem"
```

Bot 在其他机器时，可将 `listen` 改为对应网卡地址或 `0.0.0.0:10000`，并将 Bot 的实际出口 IP 加入白名单。**白名单未配置或为空会拒绝全部 gRPC 客户端**。

将 B 上的 `.bridge-token` 和 `certs/tunnel-cert.pem` 通过安全方式复制到 A 的同名位置；**私钥 `tunnel-key.pem` 仅保留在 B**。`run.sh` 自动读取令牌文件，不把它作为 shell 执行；已导出的 `BRIDGE_TOKEN` 优先。

### 2. 发送端 A

编辑 `config.toml`，把示例目标 `203.0.113.10` 改成 B 的实际公网/私网 IP。修改原始 Shred 提供方的来源白名单，以及自己的 Yellowstone 地址：

```toml
[grpc]
tunnel_address = "203.0.113.10:19000"
upstream = "127.0.0.1:10000"
token_env = "BRIDGE_TOKEN"
server_name = "bridge.example.com"
ca_file = "certs/tunnel-cert.pem"
```

`server_name` 匹配隧道证书 SAN。默认接收端脚本签发 `bridge.example.com`，无需将此域名解析到服务器；TCP 使用 `tunnel_address` 中的 IP。可用 `TLS_NAME=你的域名 ROLE=receiver ./install.sh` 初始化自定义证书名称。

默认 Jito 路由监听 A 的 `127.0.0.1:20000`，原始 Shred 路由监听 `0.0.0.0:20001`；将 Jito proxy 的 `--dest-ip-ports` 指向前者，提供方推送到后者。UDP 目标端口必须由 B 的 Bot 或桥接 UDP 路由监听。

### 3. 启动

先在 B，再在 A 执行：

```bash
./run.sh --check
./run.sh
```

指定其他配置：`./run.sh configs/config.shred-only.toml`（先编辑其中的目标地址）。只转发 Shred 不需要 gRPC 令牌和证书。所有配置路径相对于安装目录；修改后需要重启。

Linux systemd 常驻运行：

```bash
sudo ./install-service.sh
sudo systemctl restart stream-bridge-server
sudo journalctl -u stream-bridge-server -f
```

服务默认使用执行 sudo 的用户，令牌从 `.bridge-token` 加载。普通 `./run.sh` 前台运行适用于 Linux/macOS；Ctrl+C 退出。

<a id="private-network"></a>
## 内网转发

A/B 使用 `ROLE=private-source` 与 `ROLE=private-receiver` 安装，替换模板里的 `10.0.0.10`、`10.0.0.20` 和客户端 `10.0.0.30`。内网示例在 B 将 UDP `20000/20001` 转给本地 Bot 的 `21000/21001`，gRPC 客户端连接 B 的私网 `10000/TCP`。

同一区域不意味着可互通；必须已有互通 VPC、VPC Peering 或 VPN。程序使用系统路由，不创建云网络，也不会在内网失败时自动回退公网。内网仍保留 TLS、隧道令牌和客户端白名单。

<a id="clients"></a>
## Bot 客户端

安装方式、可运行示例、数据类型与集成代码见 [三语言客户端说明](clients/README_CN.md)。源码中的三个独立接收示例共用 `clients/client.toml`：

```bash
# Rust
cargo run --locked --example receive --features yellowstone -- clients/client.toml

# Go
(cd clients/go && go run ./examples/receive ../client.toml)

# Node.js / TypeScript
(cd clients/typescript && npm ci && npm run receive -- ../client.toml)
```

Rust 支持将桥接接收端嵌入 Bot；Go/TS 客户端连接已运行的 Rust 接收端。Bot 得到 Shred 原始字节、来源与接收时间，以及 Yellowstone 结构化交易/账户/slot 等消息。包管理器安装命令及当前可用状态见 [发布说明](RELEASE_NOTES.md)。

<a id="behavior"></a>
## 运行边界

- Shred 为 UDP，不保证送达；不执行重组、纠错、去重或交易解码。Bot 的有界队列满时丢弃新包并累计统计。
- Jito 鉴权、订阅和心跳由上游 proxy 负责。本工具接入其 UDP 输出，不会恢复已停用的 Jito 服务。
- gRPC 数据按客户端订阅产生。隧道断开后会重建空闲连接；原订阅需要 Bot 重连并重发过滤器，不重放历史消息。
- `BRIDGE_TOKEN` 只保护桥接隧道，不等于 Yellowstone 的 `x-token`。Bot 到接收端的链路是否加密取决于 Yellowstone 上游本身是否启用 TLS。
- 白名单依据 socket 对端 IP；NAT 后填写出口 IP，经代理时仅能看到代理 IP。详细端口、证书、版本兼容及故障说明见 [部署文档](docs/DEPLOYMENT_CN.md)。

<a id="development"></a>
## 项目结构与验证

- `src/`：Rust 桥接服务和 Bot 客户端。
- `clients/go/`、`clients/typescript/`：Go / TypeScript 客户端及示例。
- `examples/`：Rust 接收与内嵌 Bot 示例。
- `install.sh`、`setup.sh`、`run.sh`：安装、初始化与启动。
- `.github/workflows/`：测试、四平台二进制构建、客户端发布。

```bash
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
(cd clients/go && go test -race ./...)
(cd clients/typescript && npm ci && npm test)
python3 tools/smoke_clients.py
python3 -m unittest discover -s tests -p 'test_install.py'
```

端到端测试使用本地模拟 Yellowstone 和实际 source/receiver，验证三语言均收到 Shred、slot、transaction，并正常退出；生产数据源需单独配置验证。

<a id="related"></a>
## 相关项目

- [sol-trade-sdk](https://github.com/0xfnzero/sol-trade-sdk)：Solana DEX 交易构建与提交。
- [sol-parser-sdk](https://github.com/0xfnzero/sol-parser-sdk)：交易及账户事件解析。
- [sol-shred-sdk](https://github.com/0xfnzero/sol-shred-sdk)：原始 Shred 解码。
- [grpc-benchmark](https://github.com/0xfnzero/grpc-benchmark)：gRPC / Shred 延迟测速。
