use crate::common::{receive, SubscriptionConfig};
use anyhow::Result;
use serde::Deserialize;
use stream_bridge_client::ShredInput;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    pub shred: ShredInput,
    pub yellowstone: SubscriptionConfig,
}

pub async fn run(default_path: &str) -> Result<()> {
    tracing_subscriber::fmt().init();
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| default_path.into());
    let cfg: ClientConfig = toml::from_str(&std::fs::read_to_string(path)?)?;
    tokio::select! {
        result = receive(cfg.shred, cfg.yellowstone) => result,
        result = tokio::signal::ctrl_c() => { result?; Ok(()) },
    }
}
