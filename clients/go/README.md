# stream-bridge Go client

```bash
go get github.com/0xfnzero/stream-bridge/clients/go@v0.2.0
```

Import as `bridge "github.com/0xfnzero/stream-bridge/clients/go"`.

`ListenShreds(ctx, config)` delivers complete datagrams through `Packets`; monitor `Errors`, inspect `DroppedPackets()` and call `Close()` when finished. `Subscribe(ctx, config, request)` returns typed Yellowstone updates through `Recv()`, replies to server ping, and supports `UpdateFilters(request)`. Context cancellation stops the gRPC subscription.

Runnable example: `go run ./examples/receive ../client.toml` from the repository's `clients/go` directory.

[English integration guide](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/README.md) · [中文指南](https://github.com/0xfnzero/sol-stream-bridge/blob/main/clients/README_CN.md)

## Shared receiver API Keys

Version 0.2.0 supports optional bridge user keys through the `api_key` API. Set `YellowstoneConfig.APIKey` or `APIKeyEnv`. Empty values send no key. A standard Yellowstone `x-token` carrying the same bridge user key also works with a shared receiver, including from existing clients. The provider token stays on the server. IP allowlists and user quotas still apply. See the [shared mode guide](https://github.com/0xfnzero/sol-stream-bridge/blob/main/docs/SHARED.md).
