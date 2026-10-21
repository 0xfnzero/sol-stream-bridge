# React operator dashboard

[中文](DASHBOARD_CN.md) · [Shared subscriptions](SHARED.md) · [Home](../README.md)

Available on **main, unreleased**. Published v0.1.1 binaries do not include this feature. The React + TypeScript dashboard runs on shared Yellowstone receiver B; Rust serves the static assets and management API in the same process.

## Controls

- Live upstream/downstream connections, subscriptions, per-user usage, merged filters/account references, message rates, rejected requests, slow clients and upstream failures.
- Create users with random API keys; edit combined subscription/filter/account quotas and account-update permission; enable, disable, rotate keys and delete users.
- Inspect up to 200 active Subscribe streams with peer IP, start time and queued deliveries; disconnect a stream or disable a user to terminate all their streams.
- Manage the exact-IP allowlist and a separate **require API key** switch. When enabled, both checks must pass. When disabled, identities and quotas use the source IP.

Shred routes, passthrough mode, listeners, TLS, provider credentials and global upstream limits remain in TOML. This dashboard manages shared gRPC, not every forwarding topology. Rates count messages received/enqueued, not network bytes, Bot processing acknowledgements or end-to-end latency. Chart history exists only in the current page's memory.

## Start from source

Configure the source A / receiver B tunnel following [shared mode deployment](SHARED.md). In B's project directory:

```bash
# Source asset builds require Node.js >=22.12 and npm; token generation needs openssl.
./setup-dashboard.sh
```

The script creates `.dashboard-token` with mode 600, preserves an existing token, and builds missing assets. Add this table once to the shared receiver's active configuration:

```toml
[grpc.shared.dashboard]
listen = "127.0.0.1:8080"
token_env = "DASHBOARD_TOKEN"
state_file = "dashboard-state.toml"
assets_dir = "dashboard/dist"
```

A commented example is included in `config.shared-receiver.toml`. New installations may run `./setup.sh shared-receiver` to prepare a configuration and tunnel credentials; existing `config.toml` is preserved rather than switched automatically. Customize the example keys, IPs, upstream and tunnel settings, then:

```bash
cargo build --release --locked
./run.sh config.shared-receiver.toml --check
./run.sh config.shared-receiver.toml
```

Omit the filename when using `config.toml`. `run.sh` reads `.dashboard-token` as data into `DASHBOARD_TOKEN`; an exported value takes precedence. Export your own variable when changing `token_env`. Direct binary execution also needs that variable and the project/install working directory: configured paths are relative to the working directory. Open [http://127.0.0.1:8080](http://127.0.0.1:8080) and enter the token read in your own terminal.

Future Linux release packages will include `dashboard/dist` and the setup script, requiring no Node.js at runtime. The existing v0.1.1 release cannot load dashboard configuration.

## Remote administration

The listener must use loopback. Forward it through SSH:

```bash
ssh -N -L 8080:127.0.0.1:8080 user@RECEIVER_IP
```

Visit `http://127.0.0.1:8080` locally. For an occupied local port, use `-L 18080:127.0.0.1:8080` and visit port 18080. Admin HTTP travels inside SSH and does not change the Bot, provider or bridge transport. Only localhost/127.0.0.1/::1 Host values are accepted; cross-origin browser requests are rejected.

The administrator token is separate from user keys, `BRIDGE_TOKEN` and provider `x-token`. It stays in browser memory, never localStorage, cookies or URLs. Reloading/logging out requires login again. Restart to change the administrator token; user key changes apply immediately.

## User lifecycle

1. Create a user and assign quotas. A new key is returned once after creation/rotation; rotate again if lost.
2. Explicitly enable **require API key** if currently off. Creating a user does not change this switch. Nonempty TOML keys enable it on initial setup.
3. Give the user key to the Bot via source-client `api_key` or Yellowstone `x-token` / `x-api-key` metadata. Never distribute the administrator token.
4. Per-user quotas combine all that user's streams and remain subject to per-stream/global upstream limits. Account updates require explicit permission.

User key, quota, enable/disable and deletion changes terminate that user's existing subscriptions. Other users continue. Removing an IP terminates its streams. Changing authentication mode terminates all streams. Bots must handle stream termination and resubscribe. Disconnecting a stream allows reconnecting; disable the user/remove its IP to block future admissions. HTTP/2 TCP connections may remain idle after Subscribe ends, so the TCP counter need not immediately drop.

Deleting/disabling the last key **does not disable authentication**. An empty allowlist denies all gRPC clients. With key checks off, identities are `ip:<peer-IP>` and named key-user quotas do not apply; existing TOML `grpc.shared.users."ip:..."` profiles can be edited in the dashboard. Do not allocate user keys to these IP profiles.

## Persistence and backups

TOML `grpc.api_keys`, `grpc.allowed_sources` and `grpc.shared.users` seed the first run. After the first successful management save, `dashboard-state.toml` becomes authoritative for these settings, including after restart. The original TOML is not rewritten; it still controls all other settings.

The state contains plaintext user keys. It must be a regular file with no Unix group/other access (use mode 600). Writes are atomic, and multiple servers cannot manage the same state file. External edits, corruption or unsafe permissions cause save/startup failures instead of falling back to old keys. Snapshot API responses never expose keys.

Back up state, active configuration, `.dashboard-token` and tunnel credentials with restricted access. Installer upgrades preserve root TOML files, both tokens and certificates. Back up custom external state paths yourself. Deleting state and restarting reactivates the original TOML users/keys; do not use deletion as a UI reset. Also reconcile TOML policy before disabling the dashboard or downgrading binaries.

Writes include a revision. Stale edits return 409 instead of overwriting another administrator's changes. Refresh/reopen the form before retrying. A disconnected/timed-out write may have committed; refresh first. If a creation/rotation response is lost, rotate again to obtain a known key.

## Development

```bash
cd dashboard
npm ci
npm run build
npm test
npm run dev # loopback :5173, /api proxies to a running Rust dashboard on :8080
```

The frontend polls every two seconds. Persistence uses a blocking worker outside the forwarding lock. Disabled dashboards load no assets and open no management listener. Production traffic latency benchmarks have not been performed.
