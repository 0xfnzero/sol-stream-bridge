use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr};
use yellowstone_grpc_proto::{prelude::CommitmentLevel, tonic::codegen::http::Uri};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SharedConfig {
    pub dashboard: Option<super::dashboard::DashboardConfig>,
    /// Authority/TLS identity of Yellowstone. TCP always travels through the source tunnel.
    pub upstream_endpoint: String,
    pub upstream_x_token_env: Option<String>,
    pub upstream_tls_domain: Option<String>,
    pub upstream_ca_file: Option<String>,
    /// Independent of TLS on source-to-receiver tunnels.
    pub client_tls: bool,
    pub commitment: String,
    pub max_subscriptions: usize,
    pub max_subscriptions_per_key: usize,
    pub max_subscription_changes_per_second: usize,
    pub max_filters_per_key: usize,
    pub max_accounts_per_key: usize,
    pub max_filters: usize,
    pub max_accounts: usize,
    pub max_filters_per_client: usize,
    pub max_accounts_per_client: usize,
    pub max_data_filters: usize,
    pub client_queue_capacity: usize,
    pub client_queue_bytes: usize,
    pub total_queue_bytes: usize,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub allow_unfiltered: bool,
    pub allow_accounts: bool,
    pub allow_blocks: bool,
    pub allow_entries: bool,
    pub metrics_listen: Option<SocketAddr>,
    pub users: BTreeMap<String, UserLimits>,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UserLimits {
    pub max_subscriptions: Option<usize>,
    pub max_filters: Option<usize>,
    pub max_accounts: Option<usize>,
    pub allow_accounts: Option<bool>,
}
/// Only the effective per-user limits; resolving them never clones the user map.
pub struct ResolvedLimits {
    pub max_subscriptions: usize,
    pub max_filters_per_client: usize,
    pub max_accounts_per_client: usize,
    pub max_filters_per_key: usize,
    pub max_accounts_per_key: usize,
    pub allow_accounts: bool,
}
impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            dashboard: None,
            upstream_endpoint: "http://yellowstone".into(),
            upstream_x_token_env: None,
            upstream_tls_domain: None,
            upstream_ca_file: None,
            client_tls: false,
            commitment: "processed".into(),
            max_subscriptions: 64,
            max_subscriptions_per_key: 2,
            max_subscription_changes_per_second: 10,
            max_filters_per_key: 16,
            max_accounts_per_key: 256,
            max_filters: 64,
            max_accounts: 512,
            max_filters_per_client: 8,
            max_accounts_per_client: 128,
            max_data_filters: 4,
            client_queue_capacity: 64,
            client_queue_bytes: 16 * 1024 * 1024,
            total_queue_bytes: 64 * 1024 * 1024,
            max_request_bytes: 64 * 1024,
            max_response_bytes: 16 * 1024 * 1024,
            allow_unfiltered: false,
            allow_accounts: false,
            allow_blocks: false,
            allow_entries: false,
            metrics_listen: None,
            users: BTreeMap::new(),
        }
    }
}
impl SharedConfig {
    pub fn for_user(&self, name: &str) -> ResolvedLimits {
        let user = self.users.get(name);
        let filters = user.and_then(|u| u.max_filters);
        let accounts = user.and_then(|u| u.max_accounts);
        ResolvedLimits {
            max_subscriptions: user
                .and_then(|u| u.max_subscriptions)
                .unwrap_or(self.max_subscriptions_per_key),
            max_filters_per_client: filters.unwrap_or(self.max_filters_per_client),
            max_accounts_per_client: accounts.unwrap_or(self.max_accounts_per_client),
            max_filters_per_key: filters.unwrap_or(self.max_filters_per_key),
            max_accounts_per_key: accounts.unwrap_or(self.max_accounts_per_key),
            allow_accounts: user
                .and_then(|u| u.allow_accounts)
                .unwrap_or(self.allow_accounts),
        }
    }
    pub fn commitment_level(&self) -> Result<i32> {
        Ok(match self.commitment.as_str() {
            "processed" => CommitmentLevel::Processed,
            "confirmed" => CommitmentLevel::Confirmed,
            "finalized" => CommitmentLevel::Finalized,
            _ => anyhow::bail!("shared.commitment must be processed, confirmed or finalized"),
        } as i32)
    }
    pub fn validate(&self) -> Result<()> {
        self.commitment_level()?;
        if let Some(dashboard) = &self.dashboard {
            dashboard.validate()?;
        }
        let uri: Uri = self.upstream_endpoint.parse()?;
        ensure!(
            matches!(uri.scheme_str(), Some("http" | "https"))
                && uri.host().is_some()
                && uri.path() == "/"
                && uri.query().is_none()
                && !uri.authority().unwrap().as_str().contains('@'),
            "invalid shared.upstream_endpoint"
        );
        ensure!(
            uri.scheme_str() == Some("https")
                || (self.upstream_ca_file.is_none() && self.upstream_tls_domain.is_none()),
            "upstream TLS options require https"
        );
        for (value, cap) in [
            (self.max_subscriptions, 65536),
            (self.max_subscriptions_per_key, 65536),
            (self.max_subscription_changes_per_second, 1000),
            (self.max_filters, 4096),
            (self.max_filters_per_key, 65536),
            (self.max_accounts_per_key, 65536),
            (self.max_filters_per_client, 4096),
            (self.max_accounts, 65536),
            (self.max_accounts_per_client, 65536),
            (self.max_data_filters, 16),
            (self.client_queue_capacity, 4096),
            (self.client_queue_bytes, 512 * 1024 * 1024),
            (self.total_queue_bytes, 1024 * 1024 * 1024),
            (self.max_request_bytes, 1024 * 1024),
            (self.max_response_bytes, 64 * 1024 * 1024),
        ] {
            ensure!(value > 0 && value <= cap, "invalid shared limit");
        }
        ensure!(
            self.max_request_bytes >= 2,
            "max_request_bytes must fit the commitment field"
        );
        ensure!(
            self.client_queue_bytes <= self.total_queue_bytes,
            "client_queue_bytes exceeds total_queue_bytes"
        );
        if let Some(addr) = self.metrics_listen {
            ensure!(
                addr.ip().is_loopback() && addr.port() > 0,
                "metrics_listen must use a nonzero loopback port"
            );
        }
        for user in self.users.values() {
            for (value, cap) in [
                (user.max_subscriptions, 65536),
                (user.max_filters, 4096),
                (user.max_accounts, 65536),
            ] {
                ensure!(
                    value.is_none_or(|v| v > 0 && v <= cap),
                    "invalid shared user limit"
                );
            }
        }
        Ok(())
    }
}
