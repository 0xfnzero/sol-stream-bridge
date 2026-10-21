//! Run with: cargo run --example bot --features yellowstone -- config.bot.toml
#[path = "../clients/rust/examples/common/mod.rs"]
mod common;

use anyhow::Result;
use serde::Deserialize;
use stream_bridge_client::ShredInput;
use stream_bridge_server::config::{Config, Role};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BotConfig {
    bridge: Config,
    shred: ShredInput,
    yellowstone: common::SubscriptionConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.bot.toml".into());
    let cfg: BotConfig = toml::from_str(&std::fs::read_to_string(path)?)?;
    anyhow::ensure!(
        cfg.bridge.role == Role::Receiver,
        "bot bridge role must be receiver"
    );
    // Dropping these futures also shuts down their owned bridge tasks and UDP socket.
    tokio::select! {
        result = stream_bridge_server::run(cfg.bridge) => result,
        result = common::receive(cfg.shred, cfg.yellowstone) => result,
        result = tokio::signal::ctrl_c() => { result?; Ok(()) },
    }
}
