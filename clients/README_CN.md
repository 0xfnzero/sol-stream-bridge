# Bot 客户端：Rust / Go / TypeScript

[English](README.md) · [中文](README_CN.md)

包发布状态见 [发布说明](../RELEASE_NOTES.md)。Rust 可通过 crates.io、Go 可通过版本 tag、TypeScript 可通过 npm 安装；本地集成也继续支持。

三种客户端都能把数据交给 Bot：

- Shred：完整 UDP payload、发送方地址、接收时间。通过 Rust/Go channel 或 TypeScript async iterator 消费。
- Yellowstone：已解码的 `SubscribeUpdate`，包含账户、交易、slot、block、entry 等当前 protobuf 版本支持的消息。可发送完整订阅过滤器、更新过滤器，并自动回应服务端 ping。

Shred 是原始数据，不是交易对象；Bot 需要将字节包交给自己的 shred 重组/解析器。交易消息可直接从 Yellowstone 的 transaction 字段获取。客户端不会执行交易或调用交易策略，示例标注了接入策略的位置。

## 先运行接收示例

三种示例共用本目录的 `client.toml`。先启动发送端和接收端桥接，再配置：

1. `[shred].listen`：Bot 监听 UDP 的地址。发送端可以直接把 `targets` 指向这里；也可用接收端桥接的 UDP 路由转到这里。不要让 Bot 与桥接绑定同一个 UDP 地址/端口。
2. `[shred].allowed_sources`：填实际数据包来源 IP。直接接收填发送机出口/私网 IP；通过本机转发填 `127.0.0.1`。UDP 空列表允许任意来源。
3. `[yellowstone].endpoint`：接收端 gRPC `listen` 地址，例如 `http://127.0.0.1:10000`，不是 `19000` 隧道端口。
4. 接收端 `[grpc].allowed_sources` 必须允许 Bot 的 IP。这里的客户端配置不能绕过服务端白名单。
5. 上游需要认证时设置 `x_token_env`，在环境变量中提供 Yellowstone 的 x-token。该令牌与桥接两端的 `BRIDGE_TOKEN` 不同。
6. 默认订阅 slot；`transaction_accounts` 填关注账户后同时订阅相关交易。要订阅账户/块/entry 等，集成时传入完整的原生 `SubscribeRequest`。

以下命令从 `stream-bridge` 根目录开始。先把配置中的示例 IP 改为实际 IP。示例持续打印实际接收的数据；没有真实发送源时不会凭空产生数据。

### Rust

```bash
cargo run --locked --example receive --features yellowstone -- clients/client.toml
```

入口为 `examples/receive.rs`，共享接收逻辑位于 `clients/rust/examples/common/mod.rs`。初始 gRPC 建连和重试期间也会立即消费 Shred。Rust 还可把接收桥接直接嵌入 Bot：

```bash
export BRIDGE_TOKEN='<两端相同的隧道令牌>'
cargo run --locked --example bot --features yellowstone -- config.bot.toml
```

嵌入示例使用 `config.bot.toml`，在同一进程运行 gRPC receiver，并让 Shred 直接进入 Bot channel。`config.bot.toml` 是 **Bot 示例配置**，不能传给普通桥接二进制的 `--config`。

### Go

```bash
cd clients/go
go run ./examples/receive ../client.toml
```

示例源码：`clients/go/examples/receive/main.go`。要求 Go 1.24+。

### TypeScript（Node.js）

```bash
cd clients/typescript
npm ci
npm run receive -- ../client.toml
```

示例源码：`clients/typescript/examples/receive.ts`。要求 Node.js 20+。也可构建后直接运行 JavaScript：

```bash
npm run build
node dist/examples/receive.js ../client.toml
```

运行时会输出 `shred: ...`、`slot: ...`、`transaction: ...`。示例不包含签名私钥，也不发起交易。gRPC 流断开/失败时示例退出并报告错误；Ctrl+C 关闭 socket/订阅。业务 Bot 可以按自己的恢复策略重连。

## 集成到 Rust Bot

通过 crates.io 添加 `stream-bridge-client = { version = "0.1.0", features = ["yellowstone"] }`。也可在 Bot 的 Cargo.toml 中加入本地依赖（将路径换成实际位置）：

```toml
[dependencies]
stream-bridge-client = { path = "../stream-bridge/clients/rust", features = ["yellowstone"] }
```

在已有 Tokio 运行时中使用；完整错误处理与并行接收见 `clients/rust/examples/common/mod.rs`：

```rust
use stream_bridge_client::{ShredInput, ShredReceiver};
use stream_bridge_client::yellowstone::{YellowstoneSubscription, prelude::*};

let mut shreds = ShredReceiver::bind(ShredInput {
    listen: "0.0.0.0:21000".parse()?,
    allowed_sources: vec!["127.0.0.1".parse()?],
    receive_buffer: 8 * 1024 * 1024,
    channel_capacity: 4096,
}).await?;

let mut request = SubscribeRequest::default();
request.slots.insert("slots".into(), SubscribeRequestFilterSlots::default());
let mut grpc = YellowstoneSubscription::connect(
    "http://127.0.0.1:10000".into(), None, None, request,
).await?;

// 在 Bot 主循环中通过 tokio::select! 同时等待：
// shreds.recv() -> Option<Result<ShredPacket>>，使用 packet.data
// grpc.next_update() -> Result<Option<SubscribeUpdate>>，匹配 update.update_oneof
// grpc.update_filters(request).await? 替换完整过滤器
```

ShredReceiver 绑定完成后才返回，便于发现端口冲突。Drop 会停止后台接收任务；Drop YellowstoneSubscription 会关闭请求发送端及响应流。只需要 Shred 时可以不启用 `yellowstone` feature，普通桥接二进制也不必引入 Yellowstone SDK。

## 集成到 Go Bot

安装 Go 版本执行 `go get github.com/0xfnzero/stream-bridge/clients/go@v0.1.0`。本地开发可添加替换（调整绝对路径）：

```bash
go mod edit -replace=github.com/0xfnzero/stream-bridge/clients/go=/absolute/path/github.com/0xfnzero/stream-bridge/clients/go
go get github.com/0xfnzero/stream-bridge/clients/go@v0.0.0
```

```go
import bridge "github.com/0xfnzero/stream-bridge/clients/go"

shreds, err := bridge.ListenShreds(ctx, bridge.ShredConfig{
    Listen: "0.0.0.0:21000",
    AllowedSources: []string{"127.0.0.1"},
    ChannelCapacity: 4096,
})
if err != nil { return err }
defer shreds.Close()

sub, err := bridge.Subscribe(ctx, bridge.YellowstoneConfig{
    Endpoint: "http://127.0.0.1:10000",
}, bridge.DefaultRequest(nil))
if err != nil { return err }
defer sub.Close()

// 在独立 goroutine 中循环 sub.Recv()，获取 *proto.SubscribeUpdate。
// 在主循环 select shreds.Packets / shreds.Errors / ctx.Done()。
// packet.Data 是原始字节；update.GetTransaction()/GetAccount()/GetSlot() 是结构化数据。
```

`sub.UpdateFilters(*proto.SubscribeRequest)` 支持完整订阅参数；发送操作串行化，`Recv` 由单个 goroutine 调用。取消 context 或调用 Close 停止接收。完整示例包含错误传播。

## 集成到 TypeScript Bot

先在本项目 `clients/typescript` 执行 `npm ci && npm run build`，再在 Bot 项目中安装本地包：

```bash
npm install /absolute/path/stream-bridge/clients/typescript
```

```typescript
import {
  ShredReceiver, YellowstoneSubscription, defaultRequest,
} from "stream-bridge-client";

const shreds = await ShredReceiver.bind({
  listen: "0.0.0.0:21000", allowed_sources: ["127.0.0.1"], channel_capacity: 4096,
});
const grpc = await YellowstoneSubscription.connect(
  { endpoint: "http://127.0.0.1:10000" }, defaultRequest(),
);
try {
  await Promise.all([
    (async () => {
      for await (const packet of shreds) {
        // packet.data: Buffer；接入你的 shred parser。
      }
    })(),
    (async () => {
      for await (const update of grpc) {
        // update.transaction / update.account / update.slot；接入你的策略。
      }
    })(),
  ]);
} finally {
  shreds.close(); grpc.close();
}
```

使用 `parseClientConfig(parsedTomlOrJson)` 校验外部配置；未知字段、错误类型和无效 IP 会被拒绝。SDK 的 `bind`/`connect` 也执行校验，避免白名单拼写错误变为放行所有来源。

`grpc.updateFilters(SubscribeRequest)` 支持替换完整过滤器。接口支持 `for await` 或直接调用 `next()`，同一实例只应有一个读者。64 位 slot/金额等 protobuf 整数保留为字符串，不要直接转换成 JavaScript Number 后计算。npm 包已发布，可使用 `npm install stream-bridge-client`。

## 背压、断线和 TLS

- Shred channel/队列有界，默认 4096 个包；满时丢弃新到的包，Rust `dropped_packets()`、Go `DroppedPackets()`、TS `droppedPackets` 可读取累计数，不包含内核或网络丢包。耗时策略应与收包循环分离。
- Yellowstone 按流拉取并使用传输层背压，不额外维护无限响应队列。三种客户端自动回复 ping，但 Bot 必须持续读取流，不能长时间停止读取。Rust SDK 的请求发送队列为上游 SDK 的无界队列，避免无限频率更新过滤器。
- 流结束或错误会交给 Bot；客户端不会自动重放数据或掩盖订阅中断。Bot 应根据需要重新连接、重发完整过滤器，并处理断线期间的数据缺口。
- `http://` 用于明文上游。上游 Yellowstone 本身启用 TLS 时用 `https://`，必要时配置 `tls_domain` 为上游证书域名、`ca_file` 为 CA PEM 文件。这一层与桥接 A→B 的 TLS 隧道独立；`ca_file` 相对于启动目录。
- gRPC 接收消息上限为 64 MiB；更大的块消息需要按业务需求调整客户端设置。
- Rust 锁定 Yellowstone proto 12.x，Go 使用锁定的上游生成代码，TypeScript 使用兼容 grpc-js 的 Yellowstone SDK 4.x protobuf。桥接字节透传不受这些版本限制；客户端只暴露各自生成代码认识的字段，接入更新的特殊字段时需同步升级对应 SDK。

## 本地验证

```bash
# 根目录
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
(cd clients/go && go test -race ./...)
(cd clients/typescript && npm run build && npm test)
python3 tools/smoke_clients.py
```

最后一条会启动本地 mock Yellowstone、实际 source/receiver 桥接，以及三种语言的运行示例，检查每个示例都收到 Shred、slot、transaction，并验证正常退出。所有监听都在回环地址，mock 不需要真实 Solana 节点。生产环境仍需配置实际数据源。
