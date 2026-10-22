# stream-bridge

> 下文主要描述默认 TCP 透传模式；v0.2.0 新增的共享上游、用户 API Key、额度和独立客户端/上游 TLS 配置见 [shared mode / 共享模式](SHARED_CN.md).

使用 Rust 将本机的 Solana Shred 和 Yellowstone gRPC 提供给另一台服务器。**一个进程支持同时转发 Shred 和 gRPC**，两者运行于独立异步任务；可配置多路 UDP，也可只启用其中一种。

## 支持范围

- **Jito ShredStream**：接收 `jito-shredstream-proxy`（常称 jito-shred-proxy）输出的 UDP shred，再原样转发。
- **原始 Shred**：接收提供方推送到本机 IP + UDP 端口的数据，保持字节、数据包边界和发送顺序，支持多目标复制。
- **Yellowstone gRPC**：本机主动建立反向 TCP 隧道，目标服务器开放一个访问端口。该端口透明连接本机 Yellowstone，支持 HTTP/2、双向流式 Subscribe、过滤器更新、ping/pong、unary RPC、压缩、metadata 和 `x-token`；不依赖某个 Yellowstone protobuf 版本。
- 公网模板使用 TLS 与共享令牌认证；可信内网模板使用明文 TCP 与共享令牌。断线后自动重新建立空闲隧道。支持 IPv4 / IPv6、并发连接上限、UDP 来源白名单、gRPC 客户端 IP 白名单、30 秒统计日志、SIGINT / SIGTERM 退出。

Shred 不做解析、去重、纠错、补包或交易解码；Jito 的鉴权、心跳与服务订阅仍由官方 proxy 负责。本工具接入其 **UDP 输出**，不接入 Jito 的已解码 entries gRPC 服务。

本工作区的 `jito-shredstream-install/README_CN.md` 提示上游 v0.2.14 已停用旧 `shredstream` 命令；桥接依赖实际可用的 UDP 输出源，不负责恢复已停用的上游服务。

## 数据流

```text
本机 A                                             目标服务器 B
jito proxy -> UDP 127.0.0.1:20000 -> bridge -------> UDP :20000 -> shred 消费者
原始 shred -> UDP :20001 --------> bridge -------> UDP :20001 -> shred 消费者

Yellowstone :10000 <- bridge source ===TLS=====> bridge receiver :19000
                                                        ^
目标服务器上的 gRPC 客户端 -> 127.0.0.1:10000 ------------+
```

gRPC 访问按需创建上游连接，每条客户端 TCP 连接独占一条反向隧道。`idle_tunnels` 控制预连接数量，`max_connections` 控制活动 TCP 连接数；一条 HTTP/2 连接可承载多个 gRPC 订阅。

**gRPC 需要在两台服务器上运行本工具**。接收端的 `tunnel_address` 是桥接协议端口，不能指向普通 Yellowstone gRPC 服务。客户端连接接收端的 `listen`，原 Yellowstone 地址配置在发送端的 `upstream`。客户端订阅后才会产生对应数据，不会无条件订阅整条链或将消息写入任意远程 gRPC 方法。

UDP 可以直接发到目标应用监听端口，无须在目标服务器运行接收器；若要再转到本地其他端口，可使用接收端配置里的 `[[shred]]` 示例。

## Bot 客户端

已提供 **Rust、Go、TypeScript** 三套可集成客户端和可运行接收示例，详见 [客户端使用说明](../clients/README_CN.md)。Bot 可以直接读取原始 Shred 字节，以及已解码的 Yellowstone `SubscribeUpdate`，并在循环内调用自己的策略。

- Rust 库：`clients/rust/src/lib.rs`；独立接收示例 `examples/receive.rs`；内嵌桥接示例 `examples/bot.rs`。
- Go 库：`clients/go/client.go`；示例 `clients/go/examples/receive/main.go`。
- TypeScript 库：`clients/typescript/src/index.ts`；示例 `clients/typescript/examples/receive.ts`。
- 三种独立接收示例共用 `clients/client.toml`。Go/TS 连接已启动的 Rust 接收桥接；Rust 还支持将桥接嵌入 Bot 进程。

## 编译

```bash
cargo build --release --locked
```

程序为 `target/release/stream-bridge-server`。项目包含 `Cargo.lock`，使用当前 stable Rust 编译；已在 Rust 1.94.1 验证开发构建和测试。

## 同时转发 Shred 与 gRPC

1. 编辑本机的 `config.toml`，替换目标 IP、Shred 来源 IP、Yellowstone 上游地址和 TLS 服务器名称。
2. 在目标服务器编辑 `config.receiver.toml`。
3. 准备隧道 TLS 证书。已有证书可直接配置；自签证书示例：

```bash
mkdir -p certs
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -keyout certs/tunnel-key.pem -out certs/tunnel-cert.pem \
  -subj '/CN=bridge.example.com' \
  -addext 'subjectAltName=DNS:bridge.example.com'
chmod 600 certs/tunnel-key.pem
```

在目标服务器保存证书和私钥；将 **证书文件**复制到本机 `certs/tunnel-cert.pem`。本机 `server_name` 使用证书 SAN 中的名称，`tunnel_address` 仍可使用服务器 IP。相对路径均相对于启动工作目录。

4. 用 `openssl rand -hex 32` 生成一次共享令牌，两端设置为**同一个值**：

```bash
export BRIDGE_TOKEN='<同一个随机令牌，至少 32 字节>'
```

5. 先启动目标服务器：

```bash
./target/release/stream-bridge-server --config config.receiver.toml --check
./target/release/stream-bridge-server --config config.receiver.toml
```

再启动本机：

```bash
./target/release/stream-bridge-server --config config.toml --check
./target/release/stream-bridge-server --config config.toml
```

`--check` 校验 TOML、必填字段、令牌、证书/私钥文件；不会探测远端可达性或占用端口。配置中的未知字段会报错，避免拼写错误被忽略。修改配置后需要重启进程。

6. 将 Jito proxy 的 `--dest-ip-ports` 设置为 `127.0.0.1:20000`（或在已有目标列表中添加此地址）。让原始 Shred 提供方推送到本机公网 IP 的 `20001/UDP`，同时修改 `allowed_sources`。
7. 目标服务器上的 Yellowstone 客户端改连 `http://127.0.0.1:10000`，订阅参数和上游 `x-token` 保持不变。

如果上游 Yellowstone 本身启用了 TLS，客户端应使用 HTTPS 并信任上游证书，TLS SNI / 域名校验仍应匹配 **上游 Yellowstone 证书**。桥接不终止或修改这一层 TLS。

## 内网通道（同 VPC / 可互通私网）

程序原生支持私网 IP，Shred UDP 与 gRPC 反向隧道均可走内网，不需要另一种传输协议。提供一套同时转发两者的内网配置：

- `config.private-source.toml`：发送机 A，示例私网 IP 为 `10.0.0.10`。
- `config.private-receiver.toml`：接收机 B，示例私网 IP 为 `10.0.0.20`；允许 gRPC 客户端 C 的 `10.0.0.30` 访问。

把示例地址替换为实际地址。两份内网模板均设置 `insecure = true`，使用明文 TCP，无需 TLS 证书或域名。两端仍须配置同一个共享令牌。脚本安装的 `private-receiver` 只生成 `.bridge-token`，将其安全复制到 A；直接运行二进制则在两端导出相同的 `BRIDGE_TOKEN`。然后分别启动：

```bash
# B 上运行
./target/release/stream-bridge-server --config config.private-receiver.toml
# A 上运行
./target/release/stream-bridge-server --config config.private-source.toml
```

A 的 Shred 目标为 `10.0.0.20:20000/20001`，gRPC 隧道目标为 `10.0.0.20:19000`。B 将 Shred 分别交给本地 `127.0.0.1:21000/21001` 的消费者；授权客户端连接 B 的 `http://10.0.0.20:10000` 使用 Yellowstone。B 上这些网络入口仅绑定私网地址，gRPC 白名单填写客户端实际私网来源 IP；经过 NAT 则填写转换后的来源 IP。客户端到 B 若也需要 TLS，仍按前文配置上游 Yellowstone TLS。

**同一区域不等于内网互通**：两端需在互通的 VPC/子网中，或已配置 VPC Peering、云联网、VPN 等连接。此工具使用系统路由，不会自动创建 VPC/VPN 或选择“最快区域”；多网卡环境应通过系统路由确保目标私网地址走预期接口。私网不可达时，gRPC 持续重试，UDP 可能发送失败或丢失，程序不会自动改走公网。

安全组/防火墙需仅允许 A 的私网 IP 访问 B 的 `19000/TCP`、`20000/UDP`、`20001/UDP`，并允许获准客户端访问 B 的 `10000/TCP`。明文模式的隧道认证值可被抓取重放，因此只用于受控、可信的内网；公网或不可信路径保留 TLS。客户端 IP 白名单只保护 `listen`，不限制隧道端口。网络时延和流量计费以云厂商实际网络为准。

从 v0.1.1 开始，内网模板默认使用明文 TCP；v0.1.0 的内网模板开启 TLS。安装升级会保留已有 `config.toml`、令牌和证书。切换已有可信内网部署时，在两端 `[grpc]` 中显式设置 `insecure = true`，检查配置后重启两端进程；此模式忽略旧 TLS 字段。恢复 TLS 时，两端设置 `insecure = false`，并按公网模板补齐 B 的证书/私钥及 A 的证书名称/CA。

关闭隧道 TLS 可减少加解密开销；长连接中的 TLS 不会对每条消息重新握手，具体延迟收益需实测。上游支持明文时，Bot 使用 `http://IP:端口`；上游强制 TLS 时仍须使用 HTTPS，桥接不会移除上游这一层加密。

## 只转发其中一种

- **仅 Shred**：使用 `config.shred-only.toml`，不需要令牌或证书。将示例 IP 改成实际目标；多目标写入 `targets` 数组。
- **仅 gRPC**：从 `config.toml` 删除全部 `[[shred]]` 段，两端保留 `[grpc]`。
- **更多 Shred 来源**：添加 `[[shred]]` 段，各自使用不同监听地址/端口。
- **可信内网或本地明文调试**：两端 `[grpc]` 设置 `insecure = true`，此时无需证书字段，但仍需要共享令牌。公网部署使用默认 TLS。

## gRPC 客户端 IP 白名单

在目标服务器 `config.receiver.toml` 的 `[grpc]` 中配置：

```toml
[grpc]
# 其余隧道、证书等字段沿用配置文件
listen = "0.0.0.0:10000"
allowed_sources = ["127.0.0.1", "203.0.113.25", "2001:db8::25"]
```

只允许列表内的客户端 IP 访问。**省略 `allowed_sources` 或设为 `[]` 会拒绝所有 gRPC 客户端**（与 UDP 的空数组语义不同）。旧配置升级时需要显式添加白名单，修改后重启生效。默认示例仅允许本机回环地址。

检查发生在接收 TCP 连接后、分配并发名额及获取隧道之前；不匹配的连接直接关闭，不建立上游连接、不转发数据。支持 IPv4、IPv6 单个 IP，并识别 IPv4-mapped IPv6 地址；不接受 CIDR、域名或通配符。

使用 socket 实际对端 IP，不信任请求头；NAT 后客户端需要填写出口 IP。经 TCP 代理连接时只能识别代理 IP，若将代理 IP 加入白名单，需由代理限制其后方客户端。该配置仅对接收端的 gRPC `listen` 端口生效；桥接 `tunnel_address` 端口使用共享令牌认证，`insecure = false` 时另加 TLS。上游 Yellowstone 的 `x-token` 也继续生效。

## 网络与运行特性

- 目标服务器需要允许本机访问 `19000/TCP` 和 Shred 目标 UDP 端口。本机只需为外部原始 Shred 开放 UDP 入站；本机 Yellowstone 不必暴露公网。
- 接收端 gRPC `listen` 默认绑定 `127.0.0.1`。如需供其他机器访问，可改为 `0.0.0.0:10000`，在 `allowed_sources` 中添加获准客户端的出口 IP 并保留上游 Yellowstone 的认证。隧道令牌仅保护桥接端口，**不是客户端的 Yellowstone x-token**；客户端到接收端的链路是否加密取决于上游自身 TLS 或额外 TLS 入口。
- UDP 使用新的发送 socket，接收方看到的是桥接机器的出口 IP 和临时源端口；不会保留原始 IP 包头。UDP 不保证送达或最终到达顺序，繁忙目标可能丢包。每目标使用非阻塞发送；拥塞或失败时仅丢弃该目标当前的数据包并计入错误计数，不等待、不新增应用发送队列，不阻塞其他目标和入口接收。
- UDP 白名单在服务端与三语言客户端中均支持 IPv4 和对应 IPv4-mapped IPv6 地址的等价匹配。
- `receive_buffer` 是申请值，受内核限制；启动日志会输出实际 socket 缓冲区。高吞吐部署可按需要调整 Linux `net.core.rmem_max`。
- 不要配置会返回本监听端口的目标，也不要构造跨机器转发环路。程序检查明显的本机自循环，不能识别所有 NAT/路由环路。
- gRPC 不缓存、不重放历史消息。隧道或上游断开时，原订阅会失败，需要客户端自行重连和重发订阅；空闲隧道会自动恢复。`connect_timeout_ms` 控制连接/握手以及客户端等待可用隧道的期限，不限制已建立订阅时长。接收端 `tunnel_open_timeout_ms` 默认 1000ms，且不超过 `connect_timeout_ms` 的一半，限制每个候选隧道 OPEN/READY 握手（包含源端连接上游）的等待时间；超时关闭该候选，在总期限内继续尝试下一条。上游连接较慢时应同时调大这两个期限。
- TLS/TCP 隧道池使用 TCP keepalive 检测失效空闲连接；失效连接在下一次使用时被剔除，不会重放已发送的应用数据。
- 配置或监听出现致命错误时，整个进程退出并返回非零状态；UDP 单目标发送失败和 gRPC 网络断线按上述规则处理。建议使用 systemd 等服务管理器自动重启。
- `RUST_LOG=debug` 可调整日志。令牌不写入日志。

## 验证

```bash
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

集成测试覆盖 UDP 字节/边界/多目标/来源限制、配置错误、错误令牌拒绝、gRPC 白名单允许/拒绝和默认拒绝（拒绝时不消费隧道），以及通过真实 TLS 反向隧道的 tonic gRPC unary、服务端流式更新、同进程 UDP + gRPC 并行转发和 source 重连。测试使用本地服务，不需要 Solana 节点；生产 Yellowstone/Jito 服务的实际联通需配置真实地址后验证。
