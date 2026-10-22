# Shared Yellowstone subscriptions

[English](SHARED.md) · [中文](SHARED_CN.md)

Available in **v0.2.0** Linux packages and all three client SDKs. Install receiver B with `VERSION=v0.2.0 ROLE=shared-receiver ./install.sh`, then edit its `config.toml`; source builds use `cargo build --release --locked`. Use the client `api_key` API or a standard Yellowstone client with its existing `x-token` option.

Release templates live in `configs/`; run the installed server with its root `config.toml`. The source commands below use the repository-root template instead.

Add `[grpc.shared]` on receiver B to enable sharing. Source A retains its normal tunnel configuration. A receiver maintains one active upstream TCP connection and one `Subscribe` stream, merging users' requested filters. Identical filter protobufs share one upstream filter; upstream `filters` labels route updates back to the matching users with their original names restored. A user subscribing to program A does not receive updates labelled only for program B. Unknown/unlabelled upstream updates are discarded. There is no timer-based batching in the data path.

Use [config.shared-receiver.toml](../config.shared-receiver.toml) as the complete example. Run `./setup.sh receiver` to create tunnel credentials, edit the new template, then `./run.sh config.shared-receiver.toml --check` and `./run.sh config.shared-receiver.toml`. A source checkout builds the binary automatically. Copy the tunnel token and certificate to A as described in the deployment guide. Shared gRPC can coexist with the existing `[[shred]]` routes.

## Authentication

`[grpc] api_keys = { alice = "secret-a", bob = "secret-b" }` maps user names to keys. Use independent randomly generated values, for example `openssl rand -hex 32`, and restrict config-file permissions (`chmod 600`). Empty values are ignored; `{}` or a map containing only empty values disables key verification. One empty user entry does not bypass authentication when another nonempty key exists. Duplicate nonempty keys and non-visible ASCII values are rejected; keys are at most 256 bytes. Restart to apply key, permission or quota changes; this also disconnects active subscriptions.

Clients send `x-api-key` or the conventional Yellowstone `x-token`. Both headers, when present, must contain the same user key; repeated values are rejected. The client `api_key = ""` means **send no key**, not bypass server authentication. The upstream provider token is configured separately on B through `grpc.shared.upstream_x_token_env`; client metadata is never forwarded to the provider. `BRIDGE_TOKEN` still authenticates A/B tunnels.

The socket peer must also match `grpc.allowed_sources`. This remains required even with valid keys; omitted/empty lists deny everyone. With key checking disabled, per-user quotas use the canonical socket IP (`ip:127.0.0.1`, for example). Clients behind the same NAT share that quota. Per-user overrides can use this quoted identity as a TOML map key.

API Key enforcement requires shared mode. A nonempty key map without `[grpc.shared]` is rejected at startup. UDP Shred has no authentication handshake; these keys do not protect UDP. Use its IP allowlists, destination controls and network access rules.

## Quotas and permissions

- `grpc.max_connections`: receiver-wide downstream TCP connection cap, including idle connections. Source A's setting separately caps that source's upstream connections.
- `max_subscriptions`: active downstream `Subscribe` streams across the receiver; default 64. `max_subscriptions_per_key`: total active streams per identity; default 2. Authenticated requests awaiting their first subscription message also reserve these global/per-user slots. Multiple streams on the same HTTP/2 connection count separately. Per-key limits govern authenticated subscriptions, not unauthenticated idle TCP sockets.
- `max_subscription_changes_per_second`: per-identity admission/filter-change attempts per one-second window, default 10 (range 1–1000). All streams and reconnects share the budget; rejected attempts also count. Identical subscription messages and heartbeats do not consume it. Exceeding it returns `ResourceExhausted` for the requesting stream; other streams remain active. Wait at least one second before retrying.
- `max_filters_per_client` / `max_accounts_per_client`: limits on one stream; defaults 8 / 128. `max_filters_per_key` / `max_accounts_per_key`: sum across a user's streams before deduplication; defaults 16 / 256. Opening more connections cannot bypass these totals.
- `max_filters` / `max_accounts`: upstream union caps after deduplicating identical filters; defaults 64 / 512. An account reference means each entry in account/owner/include/exclude/required lists, not a unique chain address. Different filters containing the same address count it again. Set these to your provider's actual plan limits. Provider-specific category limits are not inferred automatically.
- `[grpc.shared.users.alice]`: `max_subscriptions`, `max_filters`, `max_accounts`, `allow_accounts` override the named user's limits. Filter/account overrides cap both an individual stream and that user's combined total. Server-wide upstream caps still apply.
- `allow_accounts = false` by default, including account payloads requested through block subscriptions. Enable per user only where needed. `allow_blocks` and `allow_entries` are also false by default.
- `allow_unfiltered = false` by default. Transactions require a nonempty include/required account condition or a valid signature; exclude-only filters do not qualify. Accounts require account or owner conditions. An owner/program filter can still generate substantial traffic; address-count caps are not byte-rate quotas. Slots and block metadata remain available.
- Each stream has `client_queue_capacity = 64` and `client_queue_bytes = 16777216`; all queues share `total_queue_bytes = 67108864`. Byte budgets estimate serialized data, not total process memory. A full queue terminates the affected stream with `ResourceExhausted` and releases its backlog. The global budget can also terminate streams when aggregate demand is excessive.
- `max_request_bytes = 65536` applies to individual requests and the merged upstream request. `max_response_bytes = 16777216` bounds upstream and downstream messages. `max_data_filters = 4` caps account data conditions per filter.

New admissions and filter replacements are checked before changing the union. A rejected admission leaves existing subscriptions intact. An invalid replacement terminates that user's stream. Upstream validation may impose additional unknown restrictions: if the provider rejects the union, **all** shared streams fail with `Unavailable`. Align local limits with the provider before inviting users.

For a provider limited to one connection, also set `[grpc] max_connections = 1` on source A to bound connections during teardown/reconnect. One bridge consumes one upstream connection while filters exist; idle reverse tunnels do not open Yellowstone connections. This does not account for other applications or other bridge instances using the same upstream plan. Reserve their connection allowance separately. Identical filters are deduplicated; unrelated filters are kept distinct, not automatically combined into a less selective filter.

## Transport and protocol boundaries

Use `http://IP:port` for bots on loopback or trusted private networks (`client_tls = false`). To terminate TLS for remote clients, set `client_tls = true`, provide `grpc.cert_file` / `key_file`, and use an `https://` client endpoint with the receiver's certificate identity. This is independent of A/B tunnel TLS (`grpc.insecure`) and provider TLS (`upstream_endpoint`). API keys traverse a plaintext connection as plaintext.

`upstream_endpoint` sets HTTP authority and TLS identity; the connection always travels through A's `grpc.upstream` socket. For a TLS provider, set `https://provider.example.com`, optionally `upstream_tls_domain` / `upstream_ca_file`. A private plaintext tunnel can carry that TLS session. Configure the provider token environment in B's actual service environment; for systemd, use a protected `EnvironmentFile` via a service override. Merely exporting it in an interactive shell does not set it for systemd.

Shared mode supports `Subscribe`, local `Ping`, and `GetVersion`. It uses proto 12.x and a fixed `commitment` (default `processed`); requests with a different commitment are rejected. Replay (`from_slot`), account data slices, Cuckoo filters, `SubscribeDeshred`, `SubscribeGossip` and other unary RPCs are unsupported. Omit `[grpc.shared]` for the original transparent passthrough, where each client TCP connection uses its own upstream connection and the provider handles all RPC semantics.

An upstream failure explicitly closes all downstream streams. Bots must reconnect and resend filters; no history, hidden retry or replay is provided. Slow bots must also reconnect and handle gaps. Changes are forwarded asynchronously; receipt of a client's request is not an upstream acknowledgement. New subscribers receive live updates only, without a guaranteed initial account snapshot.

## Monitoring and verification

Enable `metrics_listen = "127.0.0.1:9090"` and fetch `http://127.0.0.1:9090/metrics`. Only a nonzero loopback binding is accepted. Gauges cover connections, subscriptions, upstream connections/streams, filters and account references. Counters cover rejections, slow clients, upstream failures, updates and deliveries. All names begin with `stream_bridge_`. These are aggregate operational statistics, not per-user billing or bandwidth accounting. Logs also report statistics every 30 seconds; keys are never included.

Run `cargo test --workspace --all-features --locked`, then `python3 tools/smoke_clients.py --shared` and `python3 tools/smoke_clients.py --shared --plaintext`. Tests cover real tunnel sharing, separate users, quota rollback, denied keys/IPs, slow consumers, half-close, upstream failure/reconnect and all three runnable SDK examples. Local mocks establish behavior; production throughput and latency still require measurements against your provider and network.

## Forwarding hot path

Each upstream message reads an immutable routing snapshot without acquiring the management registry lock. Filter hashes and canonical protobufs are cached per subscription; single-filter messages reuse prepared recipient aliases. Multiple matching labels produce one delivery per subscriber with merged aliases. Queue limits remain enforced, and slow-client cleanup rebuilds the shared subscription once per batch.

Subscription changes atomically replace the routing snapshot. A dispatch already in progress, like a queued message, can complete with the previous filter names; later dispatches use the new snapshot. Revocation closes the mailbox and clears its queue even when an older snapshot still exists. Data already handed to the transport cannot be retracted. This avoids an artificial drop window during local route changes; upstream subscription changes still have the asynchronous/no-replay limits above.

The last mailbox consumer can take ownership of the protobuf payload instead of copying it; concurrent owners still receive independent data with their own filter names. Queue cleanup releases payloads outside the mailbox lock. gRPC service clones share immutable configuration instead of copying user maps. These changes do not add batching delays.

Run the repeatable in-process microbenchmark on an otherwise idle machine:

```bash
cargo test --lib --all-features --locked benchmark_shared_hot_paths -- --ignored --nocapture
```

It reports p50/p95/p99 for fanout to 64 subscribers and changing one subscription among 64 users with eight filters each. Use the same machine, build profile and workload for comparisons. It excludes socket I/O, protobuf encoding and Bot processing, so it is not an end-to-end latency or production throughput result.

To measure enqueue/dequeue of a single 128 KiB account payload, run `cargo test --lib --all-features --locked benchmark_mailbox_payload_ownership -- --ignored --nocapture` without other builds or test workloads running. Payload allocation happens before timing; the same network/encoding exclusions apply.
