# 使用示例

[English](README.md) · [中文](README_CN.md) · [文档索引](../docs/README.md)

示例需要源码检出后运行，服务端 Release 安装包只包含示例文档。运行前先配置接收端地址及 IP 白名单。示例同时接收 Shred 和 Yellowstone 数据，可将原始字节及解码消息交给 Bot 的处理函数。

- Rust：[直接接收](https://github.com/0xfnzero/sol-stream-bridge/blob/main/examples/receive.rs)、[集成 Bot](https://github.com/0xfnzero/sol-stream-bridge/blob/main/examples/bot.rs)。在仓库根目录运行 `cargo run --features yellowstone --example receive -- clients/client.toml`。
- Go：[客户端说明](../clients/go/README.md)、[接收示例](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/go/examples/receive/main.go)。
- TypeScript：[客户端说明](../clients/typescript/README.md)、[接收示例](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/typescript/examples/receive.ts)。
- [三语言集成说明](../clients/README_CN.md)：安装、配置、生命周期及队列行为。

共享模式使用接收端分配的用户 API Key；供应商 Token 留在服务端。账户更新权限及订阅额度见 [共享订阅说明](../docs/SHARED_CN.md)。

使用本地模拟上游验证三语言数据接收：

```bash
python3 tools/smoke_clients.py --shared
python3 tools/smoke_clients.py --shared --plaintext
```

测试通过真实桥接进程验证 UDP Shred 与 Yellowstone 数据交付，不发送交易。
