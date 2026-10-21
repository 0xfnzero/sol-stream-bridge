use super::config::UserLimits;
use crate::{
    client_auth::ClientAuth,
    config::{ApiKeys, Grpc},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr};
use stream_bridge_client::canonical_ip;
use yellowstone_grpc_proto::tonic::{metadata::MetadataMap, Status};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedUser {
    pub key: String,
    pub enabled: bool,
    pub limits: UserLimits,
}

/// Persisted access settings. Never serialize this object into an API response.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedState {
    pub revision: u64,
    pub require_api_key: bool,
    pub allowed_sources: Vec<IpAddr>,
    pub users: BTreeMap<String, ManagedUser>,
}
impl ManagedState {
    pub fn from_config(g: &Grpc) -> Self {
        let mut users: BTreeMap<_, _> = g
            .api_keys
            .0
            .iter()
            .map(|(name, key)| {
                (
                    name.clone(),
                    ManagedUser {
                        key: key.clone(),
                        enabled: !key.is_empty(),
                        limits: Default::default(),
                    },
                )
            })
            .collect();
        if let Some(cfg) = &g.shared {
            for (name, limits) in &cfg.users {
                users
                    .entry(name.clone())
                    .or_insert_with(|| ManagedUser {
                        key: String::new(),
                        enabled: name.starts_with("ip:"),
                        limits: Default::default(),
                    })
                    .limits = limits.clone();
            }
        }
        Self {
            revision: 0,
            require_api_key: g.api_keys.enabled(),
            allowed_sources: g.allowed_sources.clone(),
            users,
        }
    }
    pub fn validate(&self, cfg: &super::SharedConfig) -> Result<()> {
        ensure!(
            self.users.len() <= 1024 && self.allowed_sources.len() <= 1024,
            "at most 1024 users/IPs are supported"
        );
        let mut candidate = cfg.clone();
        candidate.users = self
            .users
            .iter()
            .map(|(name, user)| (name.clone(), user.limits.clone()))
            .collect();
        candidate.validate()?;
        ApiKeys(
            self.users
                .iter()
                .map(|(name, user)| (name.clone(), user.key.clone()))
                .collect(),
        )
        .validate()?;
        Ok(())
    }
}

pub struct AccessPolicy {
    pub state: ManagedState,
    auth: ClientAuth,
    has_keys: bool,
}
impl AccessPolicy {
    pub fn new(state: ManagedState) -> Result<Self> {
        let keys = ApiKeys(
            state
                .users
                .iter()
                .filter(|(_, user)| user.enabled && !user.key.is_empty())
                .map(|(name, user)| (name.clone(), user.key.clone()))
                .collect(),
        );
        Ok(Self {
            has_keys: keys.enabled(),
            auth: ClientAuth::new(&keys)?,
            state,
        })
    }
    pub fn allows(&self, peer: IpAddr) -> bool {
        self.state
            .allowed_sources
            .iter()
            .any(|ip| canonical_ip(*ip) == canonical_ip(peer))
    }
    pub fn authenticate(&self, metadata: &MetadataMap, peer: IpAddr) -> Result<String, Status> {
        if !self.allows(peer) {
            return Err(Status::permission_denied("source IP not allowed"));
        }
        let peer = canonical_ip(peer).to_string();
        let user = if self.state.require_api_key {
            // Deleting/disabling the last key must never turn authentication off.
            if !self.has_keys {
                return Err(Status::unauthenticated("no enabled API keys"));
            }
            self.auth.authenticate(metadata, &peer)?
        } else {
            format!("ip:{peer}")
        };
        if self.state.users.get(&user).is_some_and(|u| !u.enabled) {
            return Err(Status::permission_denied("user disabled"));
        }
        Ok(user)
    }
}
