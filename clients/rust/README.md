# stream-bridge-client

Rust Bot client SDK for receiving Solana Shred UDP packets and typed Yellowstone gRPC updates. This library receives data for your strategy; it does not start a bridge server or execute trades.

```toml
[dependencies]
stream-bridge-client = { version = "0.1.1", features = ["yellowstone"] }
```

```rust,no_run
use stream_bridge_client::{ShredInput, ShredReceiver};

# async fn example() -> anyhow::Result<()> {
let mut receiver = ShredReceiver::bind(ShredInput {
    listen: "0.0.0.0:21000".parse()?,
    allowed_sources: vec!["127.0.0.1".parse()?],
    receive_buffer: 8 * 1024 * 1024,
    channel_capacity: 4096,
}).await?;
while let Some(packet) = receiver.recv().await {
    let packet = packet?;
    // Feed packet.data into your Bot's shred parser.
}
# Ok(())
# }
```

Enable `yellowstone` for `yellowstone::YellowstoneSubscription`, `ClientTlsConfig` and protobuf `prelude`. Use `connect(endpoint, x_token, tls, request)`, `next_update()` and `update_filters()` to receive decoded account/transaction/slot updates. Responses to server ping are automatic. Errors and EOF are exposed, not silently replayed. Drop handles to release resources.

The UDP channel is bounded; overflow drops newest packets and increments `dropped_packets()`. Shred remains raw bytes; no reconstruction or transaction decoding is performed by this client.

Run the packaged example with a configured `client.toml` and a live bridge receiver:

```bash
cargo run --example receive_client --features yellowstone -- client.toml
```

[English guide](https://github.com/0xfnzero/stream-bridge/blob/main/clients/README.md) · [中文指南](https://github.com/0xfnzero/stream-bridge/blob/main/clients/README_CN.md)
