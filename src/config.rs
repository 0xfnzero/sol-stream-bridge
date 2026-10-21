use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::Path,
};

#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct ApiKeys(pub BTreeMap<String, String>);
impl std::fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKeys(<redacted>)")
    }
}
impl ApiKeys {
    pub fn enabled(&self) -> bool {
        self.0.values().any(|key| !key.is_empty())
    }
    pub fn validate(&self) -> Result<()> {
        let mut used = std::collections::HashSet::new();
        if self.0.len() > 1024 {
            bail!("at most 1024 API key entries are supported");
        }
        for key in self.0.values() {
            stream_bridge_client::auth::validate_api_key(key)?;
            if !key.is_empty() && !used.insert(key) {
                bail!("API keys must be unique per user");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub role: Role,
    #[serde(default)]
    pub shred: Vec<Shred>,
    pub grpc: Option<Grpc>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Source,
    Receiver,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shred {
    pub name: String,
    pub listen: SocketAddr,
    pub targets: Vec<SocketAddr>,
    #[serde(default)]
    pub allowed_sources: Vec<IpAddr>,
    #[serde(default = "receive_buffer")]
    pub receive_buffer: usize,
}
fn receive_buffer() -> usize {
    8 * 1024 * 1024
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grpc {
    pub tunnel_address: String,
    pub listen: Option<SocketAddr>,
    pub upstream: Option<String>,
    pub token_env: String,
    /// Receiver client allowlist. Missing/empty means deny every client.
    #[serde(default)]
    pub allowed_sources: Vec<IpAddr>,
    #[serde(default)]
    pub api_keys: ApiKeys,
    /// Enable one shared Yellowstone Subscribe stream instead of TCP passthrough.
    pub shared: Option<crate::shared::SharedConfig>,
    #[serde(default)]
    pub insecure: bool,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
    pub ca_file: Option<String>,
    pub server_name: Option<String>,
    #[serde(default = "pool_size")]
    pub idle_tunnels: usize,
    #[serde(default = "max_connections")]
    pub max_connections: usize,
    #[serde(default = "reconnect_ms")]
    pub reconnect_ms: u64,
    #[serde(default = "connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// Maximum time to open one candidate; also capped at half the total wait.
    #[serde(default = "tunnel_open_timeout_ms")]
    pub tunnel_open_timeout_ms: u64,
}
fn pool_size() -> usize {
    8
}
fn max_connections() -> usize {
    256
}
fn reconnect_ms() -> u64 {
    1000
}
fn connect_timeout_ms() -> u64 {
    10000
}
fn tunnel_open_timeout_ms() -> u64 {
    1000
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let c: Self =
            toml::from_str(&std::fs::read_to_string(path)?).map_err(|error: toml::de::Error| {
                // TOML diagnostics can quote the source line, including literal API keys.
                anyhow::anyhow!(
                    "invalid configuration syntax or field types at {:?}",
                    error.span()
                )
            })?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        if self.shred.is_empty() && self.grpc.is_none() {
            bail!("configure at least one [[shred]] route or [grpc]");
        }
        for route in &self.shred {
            if route.targets.is_empty() || route.receive_buffer == 0 || route.listen.port() == 0 {
                bail!(
                    "route {} needs targets, a nonzero port and receive_buffer",
                    route.name
                );
            }
            for target in &route.targets {
                if target.port() == 0 || target.ip().is_unspecified() || target.ip().is_multicast()
                {
                    bail!("invalid UDP target: {target}");
                }
                if target.port() == route.listen.port()
                    && (target.ip() == route.listen.ip()
                        || route.listen.ip().is_unspecified() && target.ip().is_loopback())
                {
                    bail!("route {} forwards to itself", route.name);
                }
            }
        }
        if let Some(g) = &self.grpc {
            g.token()?;
            g.api_keys.validate()?;
            if let Some(shared) = &g.shared {
                if self.role != Role::Receiver {
                    bail!("grpc.shared belongs on the receiver");
                }
                shared.validate()?;
                if shared.client_tls && (g.cert_file.is_none() || g.key_file.is_none()) {
                    bail!("shared.client_tls requires grpc.cert_file and grpc.key_file");
                }
            } else if g.api_keys.enabled() {
                bail!("configure [grpc.shared] to enable client API key authentication");
            }
            if self.role == Role::Source && g.api_keys.enabled() {
                bail!("grpc.api_keys belongs on the receiver, not the source");
            }
            if g.idle_tunnels == 0
                || g.idle_tunnels > 1024
                || g.max_connections == 0
                || g.max_connections > 65536
                || g.reconnect_ms == 0
                || g.connect_timeout_ms == 0
                || g.tunnel_open_timeout_ms == 0
            {
                bail!("invalid gRPC limits/timeouts (idle_tunnels: 1..1024; max_connections: 1..65536)");
            }
            check_address(&g.tunnel_address)?;
            match self.role {
                Role::Source => {
                    check_address(
                        g.upstream
                            .as_deref()
                            .context("source requires grpc.upstream")?,
                    )?;
                    if !g.insecure && g.server_name.as_deref().unwrap_or("").is_empty() {
                        bail!("TLS source requires grpc.server_name");
                    }
                }
                Role::Receiver => {
                    if g.listen.context("receiver requires grpc.listen")?.port() == 0 {
                        bail!("grpc.listen needs a nonzero port");
                    }
                    if !g.insecure && (g.cert_file.is_none() || g.key_file.is_none()) {
                        bail!("TLS receiver requires cert_file and key_file");
                    }
                }
            }
        }
        Ok(())
    }
}
fn check_address(s: &str) -> Result<()> {
    let (host, port) = s
        .rsplit_once(':')
        .context("expected host:port, without http:// or https://")?;
    if host.is_empty() || s.contains('/') || port.parse::<u16>().unwrap_or(0) == 0 {
        bail!("invalid address {s}");
    }
    Ok(())
}
impl Grpc {
    pub fn allows_client(&self, ip: IpAddr) -> bool {
        use stream_bridge_client::canonical_ip;
        let ip = canonical_ip(ip);
        self.allowed_sources
            .iter()
            .any(|allowed| canonical_ip(*allowed) == ip)
    }
    pub fn token(&self) -> Result<String> {
        let token = std::env::var(&self.token_env)
            .with_context(|| format!("set environment variable {}", self.token_env))?;
        if token.len() < 32 {
            bail!("tunnel token must contain at least 32 bytes");
        }
        Ok(token)
    }
}
