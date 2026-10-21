# Bot clients

[English](README.md) · [中文](README_CN.md)

Rust, Go and TypeScript clients expose raw Shred packets and decoded Yellowstone `SubscribeUpdate` messages. They support complete subscription filters, filter replacement and automatic server-ping replies. Read [release notes](../RELEASE_NOTES.md) for package versions. Rust and npm clients are published as `stream-bridge-client`.

## Run the examples

Start the source and receiver bridge first. Configure `client.toml`:

- `shred.listen`: Bot UDP listener. Send directly here or forward from the receiver. Do not bind the bridge and Bot to the same socket.
- `shred.allowed_sources`: actual source IP; use loopback if receiving via a local forwarder. Empty UDP lists allow all.
- `yellowstone.endpoint`: receiver's gRPC listener, usually `http://127.0.0.1:10000`, not the tunnel port.
- The receiver's `grpc.allowed_sources` must include the Bot IP.
- `yellowstone.x_token_env`: optional environment variable holding the upstream Yellowstone token, distinct from `BRIDGE_TOKEN`.
- `transaction_accounts`: default empty means slots only; add accounts to subscribe to matching transactions too.

From the repository root:

```bash
cargo run --locked --example receive --features yellowstone -- clients/client.toml
(cd clients/go && go run ./examples/receive ../client.toml)
(cd clients/typescript && npm ci && npm run receive -- ../client.toml)
```

The examples print actual received `shred:`, `slot:` and `transaction:` messages. They do not need a wallet key and do not trade. Without an upstream source, no chain data is produced. Shred consumption starts immediately, including while the initial gRPC connection is pending. Ctrl+C closes resources; stream errors/EOF are reported so the Bot can choose its recovery policy.

Rust can also embed the receiver:

```bash
cargo run --locked --example bot --features yellowstone -- config.bot.toml
```

The embedded example starts a receiver and delivers Shred directly to a Bot channel. `config.bot.toml` belongs to this example, not the regular bridge CLI.

## Rust integration

Install from crates.io:

```toml
[dependencies]
stream-bridge-client = { version = "0.1.0", features = ["yellowstone"] }
```

For local development use `path = "../stream-bridge/clients/rust"` instead of `version`. Main APIs:

```rust
use stream_bridge_client::{ShredInput, ShredReceiver};
use stream_bridge_client::yellowstone::{YellowstoneSubscription, prelude::*};
```

`ShredReceiver::bind(config).await?` binds before returning. `recv().await` produces `Option<Result<ShredPacket>>`; use `packet.data`, `peer`, and `received_at`. The bounded channel drops newest packets on overflow; inspect `dropped_packets()`.

`YellowstoneSubscription::connect(endpoint, x_token, tls, request).await?` opens a typed subscription. `next_update().await?` returns `Option<SubscribeUpdate>` and automatically replies to ping. `update_filters(request).await?` replaces complete filters. Use `tokio::select!` for both inputs. Dropping the handles stops reception. The Rust examples share their concurrent receive loops in [common/mod.rs](rust/examples/common/mod.rs).

Only raw Shred is needed? Omit the optional `yellowstone` feature. The regular bridge binary also works without it.

## Go integration

Go 1.24+. Install the versioned Go module:

```bash
go get github.com/0xfnzero/stream-bridge/clients/go@v0.1.0
```

```go
import bridge "github.com/0xfnzero/stream-bridge/clients/go"

shreds, err := bridge.ListenShreds(ctx, bridge.ShredConfig{
    Listen: "0.0.0.0:21000", AllowedSources: []string{"127.0.0.1"},
    ChannelCapacity: 4096,
})
if err != nil { return err }
defer shreds.Close()

sub, err := bridge.Subscribe(ctx, bridge.YellowstoneConfig{
    Endpoint: "http://127.0.0.1:10000",
}, bridge.DefaultRequest(nil))
if err != nil { return err }
defer sub.Close()
```

Select over `shreds.Packets`, `shreds.Errors` and `ctx.Done()`. `packet.Data` is the original payload. In one separate goroutine call `sub.Recv()` to get protobuf updates; `GetTransaction()`, `GetAccount()`, `GetSlot()` expose data. `UpdateFilters(*proto.SubscribeRequest)` supports full native filters and serializes sends. Cancel context or call Close to stop. Full example: [receive/main.go](go/examples/receive/main.go).

For local development use a Go `replace` directive pointing `github.com/0xfnzero/stream-bridge/clients/go` at the local `clients/go` directory. Go publication uses a Git tag (`clients/go/v0.1.0`), not an npm-style package upload.

## TypeScript integration

Node.js 20+:

```bash
npm install stream-bridge-client
```

For local development, build `clients/typescript` with `npm ci && npm run build`, then run `npm install /absolute/path/stream-bridge/clients/typescript` in your Bot project.

```typescript
import { ShredReceiver, YellowstoneSubscription, defaultRequest } from "stream-bridge-client";

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
        // Feed packet.data (Buffer) to your shred parser.
      }
    })(),
    (async () => {
      for await (const update of grpc) {
        // Invoke your strategy with update.transaction / update.account / update.slot.
      }
    })(),
  ]);
} finally {
  shreds.close(); grpc.close();
}
```

`parseClientConfig(parsedTomlOrJson)` validates both configuration sections and rejects unknown keys before startup. The SDK also validates each configuration passed directly to `bind`/`connect`; misspelled allowlist fields are rejected. `updateFilters(SubscribeRequest)` replaces full filters. Use one reader per instance. Keep protobuf 64-bit integer strings as strings or BigInt, not Number. The package ships `.d.ts` declarations, compiled JavaScript, examples, a config template and a `stream-bridge-client` CLI. Full example: [receive.ts](typescript/examples/receive.ts).

## Backpressure, TLS and compatibility

- Shred is raw UDP, not decoded transactions. Supply your own shred reconstruction/parser, or consume Yellowstone transactions instead.
- UDP queues default to 4096 packets; full queues drop the newest packet. Counters exclude kernel/network loss. Keep strategy work off receive loops.
- gRPC responses are pulled with transport backpressure. Keep reading so ping replies continue. Rust's upstream SDK has an unbounded request queue; do not enqueue filter changes without a bound.
- EOF/errors are exposed. Clients do not silently reconnect, replay or hide gaps; reopen and resend filters according to your Bot's recovery policy.
- Use `http://IP:port` when the upstream supports plaintext; `https://` authenticates a TLS upstream Yellowstone service. `tls_domain` overrides its certificate domain; `ca_file` supplies a private CA PEM, relative to the working directory. These settings are independent of the bridge tunnel: private templates use plaintext TCP, while public templates use TLS.
- Received gRPC messages are limited to 64 MiB.
- Rust uses proto 12.x; Go pins upstream generated code; TypeScript uses grpc-js-compatible SDK 4.x protobuf. Unknown newer fields require upgrading the corresponding client dependency; the transparent bridge itself is not tied to those protobuf versions.

## Verification

```bash
cargo test --all-features --locked
(cd clients/go && go test -race ./...)
(cd clients/typescript && npm run build && npm test)
python3 tools/smoke_clients.py
```

The smoke test runs a local mock Yellowstone, real source/receiver bridges and all three example programs, verifying Shred, slot, transaction and clean shutdown. Live Solana data requires your own configured upstream.
