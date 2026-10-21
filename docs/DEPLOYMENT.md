# Deployment reference

> This reference describes default TCP passthrough. For unreleased shared subscriptions, user API Keys, quotas and independent client/provider TLS, see [shared mode / 共享模式](SHARED.md).

[English](DEPLOYMENT.md) · [中文](DEPLOYMENT_CN.md)

## Ports and data paths

Source A accepts Jito output on loopback UDP or raw provider packets on a configured UDP interface, then copies each datagram to one or more targets. Receiver B may consume the UDP directly or relay it to a Bot's local UDP socket.

For gRPC, A establishes authenticated reverse TCP connections to B's `grpc.tunnel_address`. Bots connect to B's `grpc.listen`; each Bot TCP connection uses one tunnel to A's `grpc.upstream`. HTTP/2 and protobuf bytes are not decoded by the bridge. Do not point the tunnel address at a normal Yellowstone service.

Typical firewall rules:

- A → B: TCP 19000 for the reverse tunnel; UDP 20000/20001 for Shred.
- Authorized Bot → B: TCP 10000, if the Bot is remote.
- Provider → A: the configured raw Shred UDP ingress.
- The source's Yellowstone port need not be public.

Allow private IPs when using VPC routes. A region alone does not establish network connectivity. The private templates bind B's ingress to its private IP and forward UDP to local ports 21000/21001. They use plaintext TCP for the gRPC tunnel (`insecure = true` on both ends), intended for controlled, trusted networks. Public-network templates keep TLS enabled. Shred forwarding uses ordinary UDP in both modes.

## Authentication and TLS

Public `receiver` setup creates a random `.bridge-token` and TLS certificate/private key. Copy only the token and certificate to A; do not copy B's private key. Default certificate lifetime is 365 days. Renew before expiration and update the source's trusted certificate when replacing a self-signed certificate. `private-receiver` setup creates only `.bridge-token`; copy that token securely to the private source.

`grpc.server_name` on A must match the certificate SAN. Public-CA certificates can use native public trust roots without `ca_file`; private/self-signed certificates require it. `--check` validates files and syntax, not remote connectivity.

The A→B tunnel and the original Bot→Yellowstone session have independent TLS settings. The bridge does not terminate or alter the original session. Use `http://IP:port` for a plaintext upstream; if the upstream requires TLS, use `https://` and validate its certificate and name, even through a plaintext bridge tunnel. A plaintext upstream leaves the Bot→B leg plaintext unless separately protected.

With `insecure = true`, the tunnel has no TLS handshake or data encryption, but its shared-token authentication value travels in plaintext and can be captured and replayed. Restrict the tunnel port to the source using the private network's firewall; the Bot IP allowlist does not protect this port. Use TLS for public or untrusted paths. TLS on persistent connections does not perform a handshake per message; removing it reduces encryption work but does not guarantee a specific latency improvement.

`grpc.allowed_sources` applies only to B's Bot-facing listener. Empty/missing means deny all. It accepts exact IPv4/IPv6 addresses, including IPv4-mapped IPv6 equivalents; no CIDR, DNS names or wildcards. The tunnel port requires the shared token and uses TLS when `insecure = false`. Proxy headers do not override the socket peer IP.

UDP `allowed_sources` has different semantics: an empty list accepts any source. IPv4-mapped IPv6 addresses match their IPv4 equivalents in the server and all clients. Use explicit provider/source IPs for controlled deployments.

## Configuration and operation

Templates are under `configs/` in installed releases and in the repository root for source checkouts. The active installed configuration is root `config.toml`. `run.sh [path.toml] [--check]` changes into the install directory and loads `.bridge-token` without shell evaluation. Set custom token environment variables externally if using a `token_env` other than `BRIDGE_TOKEN`.

To switch an existing installation's role, back up its active config and copy the desired `configs/config.*.toml` to `config.toml`. Setup/upgrade never silently replaces an active config. Run `./setup.sh receiver` for TLS credentials, or `./setup.sh private-receiver` for a token only.

Private templates default to plaintext TCP starting with v0.1.1; v0.1.0 templates enable TLS. Existing private installations keep their active config and certificates on upgrade. To switch a trusted private deployment, set `[grpc] insecure = true` on both ends, check the config and restart both processes. TLS fields are ignored in this mode. To enable TLS again, use `insecure = false` on both ends, supply B's `cert_file` / `key_file` and A's `server_name` / trusted CA, then check and restart.

`idle_tunnels` controls preconnected TCP sockets. `max_connections` limits concurrent client TCP connections, not individual HTTP/2 subscriptions. `connect_timeout_ms` bounds connect/handshake/wait-for-source operations; established subscriptions have no application timeout. Receiver `tunnel_open_timeout_ms` (default 1000 ms, capped at half of `connect_timeout_ms`) limits each candidate OPEN/READY exchange, including the source connecting to its upstream. Timed-out candidates are closed and the next candidate is tried within the total deadline. Increase both timeouts if the upstream needs longer to accept a connection. TCP keepalive detects dead peers, and stale idle tunnels are discarded before forwarding client data. No application data is replayed.

Fatal bind/config errors terminate the process. UDP destination errors are counted while later packets continue. Source network failures trigger reconnect attempts. Use systemd for process restarts; existing subscriptions still need client-side reconnection and gap handling.

## UDP and resource limits

Each original datagram is forwarded unchanged. The receiver sees A's egress IP and a newly allocated source port, not the original provider IP/port. There is no delivery guarantee, FEC, reordering or deduplication. Avoid forwarding loops; only obvious local self-loops are rejected by validation.

The default requested UDP receive buffer is 8 MiB. The kernel may cap it; actual size is logged. Tune Linux `net.core.rmem_max` if necessary. Target sends are nonblocking. A busy or failed target drops only its copy of the current packet and increments the error counter; it never pauses other targets or ingress reception. No application send queue is added. UDP stats are logged every 30 seconds.

Bot raw-Shred queues default to 4096 packets and drop the newest packet when full. Yellowstone clients expose stream errors/EOF and do not silently reconnect/replay. Keep strategy computation off receive loops. Client message limits and protobuf compatibility are documented in [client integration](../clients/README.md).

## Service management

`sudo ./install-service.sh` validates the config as the eventual service user, installs a systemd unit and starts it. The default user is `SUDO_USER`; override with `SERVICE_USER` if needed. That user must read the config/token/certificates. Do not run a second foreground instance on the same ports.

Use `systemctl stop/start/restart stream-bridge-server` and `journalctl -u stream-bridge-server -f`. After upgrading, restart to load the new binary. Previous managed installations remain in sibling `.backup.*` directories for rollback; they include private configuration and must be protected.
