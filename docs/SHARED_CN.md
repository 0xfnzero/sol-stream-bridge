# Yellowstone 共享订阅（尚未发布）

[English](SHARED.md) · [中文](SHARED_CN.md)

本功能在 `main` 源码中，已发布的 v0.1.1 二进制和客户端包不包含这些新配置/API。服务端使用 `cargo build --release --locked` 编译；新 `api_key` API 使用本地 SDK，已有标准 Yellowstone 客户端也可通过原来的 `x-token` 参数接入。

在接收端 B 增加 `[grpc.shared]` 即启用共享模式，发送端 A 保留原来的隧道配置。一个 receiver 只维持一条活动上游 TCP 连接和一条 `Subscribe` 流，合并各用户过滤器。内容相同的过滤器在上游去重；通过上游返回的 `filters` 标签分发给匹配用户，并恢复各自原始过滤器名称。只订阅 A 程序的用户不会收到仅带 B 程序标签的数据；无标签、未知标签的数据不广播。数据路径没有定时攒批等待。

完整示例见 [config.shared-receiver.toml](../config.shared-receiver.toml)。先用 `./setup.sh receiver` 生成隧道凭据，修改新模板后执行 `./run.sh config.shared-receiver.toml --check`、`./run.sh config.shared-receiver.toml`。源码目录下脚本会自动编译。按部署说明把隧道令牌和证书复制给 A。共享 gRPC 可以与原有 `[[shred]]` 同时运行。

## API Key 与 IP 白名单

服务端 `[grpc] api_keys = { alice = "secret-a", bob = "secret-b" }` 将用户名映射到各自的 Key。建议分别运行 `openssl rand -hex 32` 生成随机值，并用 `chmod 600` 限制配置文件权限。空值会被忽略；`{}` 或所有值都为空时，关闭 API Key 验证。只要还有一个非空 Key，空值用户就不能免认证。禁止重复非空 Key，最多 256 个可见 ASCII 字节。修改 Key、权限或额度需要重启，会断开现有订阅。

客户端发送 `x-api-key` 或标准 Yellowstone `x-token` 均可；两者同时提供时必须是相同的用户 Key，不接受重复头。客户端 `api_key = ""` 表示不发送 Key，不能绕过服务端已经开启的验证。供应商的 Token 单独通过 B 的 `grpc.shared.upstream_x_token_env` 配置，客户端 metadata 不会透传给供应商。`BRIDGE_TOKEN` 仍只认证 A/B 隧道。

客户端实际 socket 来源 IP 还必须在 `grpc.allowed_sources` 中，两个条件同时满足才允许使用。省略或空 IP 列表拒绝所有客户端。关闭 Key 验证后，按规范化的来源 IP 分配用户额度，例如 `ip:127.0.0.1`；同一 NAT 出口共享额度。此时可用带引号的 IP 身份作为 TOML 用户配置名称。

API Key 验证需要共享模式；没有 `[grpc.shared]` 却配置非空 Key 会在启动时被拒绝。原始 UDP Shred 没有认证握手，API Key 不保护 UDP，继续使用 UDP 来源白名单、目标配置和网络访问控制。

## 连接、额度与账户权限

- `grpc.max_connections`：接收端所有下游 TCP 连接上限，包括空闲连接；发送端 A 的同名参数单独限制该发送端的上游连接数。
- `max_subscriptions`：全局活动 `Subscribe` 流上限，默认 64。`max_subscriptions_per_key`：每用户活动流上限，默认 2。已认证但还在等待首个订阅消息的请求，也占用上述全局及用户名额。同一 HTTP/2 连接上的多个订阅分别计数。按 Key 限制的是通过认证的订阅数，不是尚未认证的空闲 TCP socket。
- `max_filters_per_client` / `max_accounts_per_client`：单条订阅上限，默认 8 / 128。`max_filters_per_key` / `max_accounts_per_key`：用户所有订阅的累计值，去重前计算，默认 16 / 256；多开连接不能绕过累计额度。
- `max_filters` / `max_accounts`：相同过滤器去重后的上游合并额度，默认 64 / 512。账户数按 account、owner、include、exclude、required 列表的显式引用次数计算，不是唯一链上地址数；不同过滤器重复引用同一地址仍分别计数。按供应商实际套餐设置这些值，程序不会自动推断其分类限制。
- `[grpc.shared.users.alice]` 中的 `max_subscriptions`、`max_filters`、`max_accounts`、`allow_accounts` 覆盖该用户策略。过滤器/账户覆盖值同时限制单条订阅和该用户所有订阅累计值；上游全局上限仍生效。
- 默认 `allow_accounts = false`，包括通过 block 请求账户数据的路径。可单独给某用户开启。`allow_blocks`、`allow_entries` 默认也为 false。
- 默认 `allow_unfiltered = false`：交易必须带 include/required 账户条件或合法 signature，只有 exclude 不算；账户订阅必须带 account 或 owner。即使限制了账户数，owner/程序过滤仍可能产生大量数据；这些额度不等于带宽速率限制。slot 和 block metadata 仍可订阅。
- 每条流默认 `client_queue_capacity = 64`、`client_queue_bytes = 16777216`，所有队列共享 `total_queue_bytes = 67108864`。字节预算按序列化大小估算，不代表进程 RSS 上限。队列满时向该流返回 `ResourceExhausted`，立即释放积压；全局预算耗尽也可能关闭其他无法入队的流。
- `max_request_bytes = 65536` 同时限制单条请求和上游合并请求；`max_response_bytes = 16777216` 限制收发消息；`max_data_filters = 4` 限制单个账户过滤器的数据条件数量。

新订阅和过滤器替换都会先校验再修改合并结果。超额的新请求不会改变其他用户的订阅；已有流提交非法更新时关闭该流。供应商可能还有本地未知的限制：一旦上游拒绝合并请求，**所有共享流**都会收到 `Unavailable`，因此开放服务前应对齐供应商规则。

供应商严格限一条连接时，也在发送端 A 设置 `[grpc] max_connections = 1`，限制断开/重连过程中的上游连接数量。有过滤器时一个 bridge 使用一个上游连接；空闲反向隧道不占 Yellowstone 连接。其他程序或其他 bridge 实例占用的连接不在此预算内，需要自行预留。内容相同的过滤器会去重，不同的过滤器保持独立，不会自动合并成更宽泛的条件。

## 传输方式与兼容范围

本机/可信内网 Bot 使用 `http://IP:端口`，即 `client_tls = false`。需要为远程客户端开启 TLS 时设为 true，配置 `grpc.cert_file` / `key_file`，客户端使用 `https://` 并验证接收端证书。这与 A/B 隧道的 `grpc.insecure`、上游的 `upstream_endpoint` 相互独立。明文链路上的 API Key 也是明文。

`upstream_endpoint` 指定 HTTP authority/TLS 身份，实际连接始终通过 A 的 `grpc.upstream`。供应商需要 TLS 时填 `https://provider.example.com`，可选 `upstream_tls_domain` / `upstream_ca_file`；私网明文隧道也可承载该 TLS 会话。供应商 Token 环境变量需要设置在 B 的实际进程环境中；systemd 可用服务 override 引入权限受限的 `EnvironmentFile`，仅在交互终端 export 不会传入 systemd 服务。

共享模式支持 `Subscribe`、本地 `Ping`、`GetVersion`，使用 proto 12.x。`commitment` 固定，默认 processed；客户端指定其他值会被拒绝。不支持 `from_slot` 回放、账户数据切片、Cuckoo 过滤器、`SubscribeDeshred`、`SubscribeGossip` 及其他 unary RPC。省略 `[grpc.shared]` 即原有透明透传，每条下游 TCP 连接独占一条上游连接，完整 RPC 行为由供应商处理。

上游断线会明确关闭全部下游流，Bot 应重连并重新提交过滤器，不缓存历史、不静默重试、不重放。慢客户端被断开也要处理数据缺口。订阅修改异步发送给上游；本地收到请求不表示上游已经确认生效。新用户接收实时数据，不保证初始账户快照。

## 监控与验证

开启 `metrics_listen = "127.0.0.1:9090"` 后访问 `http://127.0.0.1:9090/metrics`，仅允许非零端口的 loopback 监听。Gauge 包括下游连接数、订阅数、上游连接/流数量、过滤器数和账户引用数；Counter 包括拒绝次数、慢客户端、上游失败、接收更新和分发消息数，名称均以 `stream_bridge_` 开头。这是整体运行统计，不提供按用户计费或带宽用量统计。每 30 秒也输出统计日志，不记录 Key。

执行 `cargo test --workspace --all-features --locked`，以及 `python3 tools/smoke_clients.py --shared`、`python3 tools/smoke_clients.py --shared --plaintext`。测试覆盖真实隧道共享、多用户分发、额度回滚、Key/IP 拒绝、慢用户隔离、半关闭、上游断线重连，以及三语言可运行示例。本地 mock 验证功能行为；生产吞吐、带宽和延迟仍需在实际上游和网络环境中测量。
