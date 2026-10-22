# Examples

[English](README.md) · [中文](README_CN.md) · [Documentation](../docs/README.md)

Use a source checkout to run these examples (the server release archive contains documentation only). Configure the receiver endpoint and allowed IPs before running an example. Shred reception and Yellowstone subscriptions run together; the examples deliver bytes and typed updates to your own handler.

- Rust: [receive](https://github.com/0xfnzero/sol-stream-bridge/blob/main/examples/receive.rs), [bot integration](https://github.com/0xfnzero/sol-stream-bridge/blob/main/examples/bot.rs); run `cargo run --features yellowstone --example receive -- clients/client.toml` from the repository root.
- Go: [client guide](../clients/go/README.md) and [receive example](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/go/examples/receive/main.go).
- TypeScript: [client guide](../clients/typescript/README.md) and [receive example](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/typescript/examples/receive.ts).
- [Three-language integration guide](../clients/README.md): installation, configuration, lifecycle and queue behavior.

For shared mode, use a user API Key from the receiver. The provider token belongs on the server. See [shared subscriptions](../docs/SHARED.md) for account-update permissions and subscription limits.

To verify all three languages with a local mock upstream:

```bash
python3 tools/smoke_clients.py --shared
python3 tools/smoke_clients.py --shared --plaintext
```

These tests send no trades. They check UDP Shred and Yellowstone delivery through real bridge processes.
