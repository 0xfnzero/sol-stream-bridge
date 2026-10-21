# stream-bridge-client (TypeScript)

Typed Bot client for Solana Shred UDP and Yellowstone gRPC.

```bash
npm install stream-bridge-client@0.1.1
```

Exports `ShredReceiver`, `YellowstoneSubscription`, `defaultRequest`, `SubscribeRequest` and `SubscribeUpdate`. UDP reception uses a bounded async iterator; Yellowstone returns decoded updates, supports filter changes and replies to ping. Call `close()` to release resources.

```typescript
import { YellowstoneSubscription, defaultRequest } from "stream-bridge-client";
const sub = await YellowstoneSubscription.connect(
  { endpoint: "http://127.0.0.1:10000" }, defaultRequest(),
);
try { for await (const update of sub) console.log(update.slot); }
finally { sub.close(); }
```

Run the included receiver with `npx stream-bridge-client /path/to/client.toml`. A template is included in this package; edit listener addresses and source allowlists first. Run the Rust bridge receiver separately. The upstream source must be configured and active.

[English integration guide](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/README.md) · [中文指南](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/README_CN.md)

Use `parseClientConfig(parsedTomlOrJson)` for configuration loaded from files. Unknown keys and invalid types are rejected; `bind` and `connect` also validate direct API input. IP allowlists match all equivalent IPv4-mapped IPv6 forms.
