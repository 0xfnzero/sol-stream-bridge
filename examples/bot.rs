//! Run with: cargo run --example bot --features yellowstone -- config.bot.toml
use anyhow::{Context, Result};
use serde::Deserialize;
use stream_bridge_server::{
    bot::{
        yellowstone::{prelude::*, ClientTlsConfig, YellowstoneSubscription},
        ShredInput, ShredReceiver,
    },
    config::Config,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BotConfig {
    bridge: Config,
    shred: ShredInput,
    yellowstone: SubscriptionConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscriptionConfig {
    endpoint: String,
    x_token_env: Option<String>,
    /// Optional upstream certificate domain when connecting through a local endpoint.
    tls_domain: Option<String>,
    ca_file: Option<String>,
    #[serde(default)]
    transaction_accounts: Vec<String>,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.bot.toml".into());
    let cfg: BotConfig = toml::from_str(&std::fs::read_to_string(path)?)?;
    anyhow::ensure!(
        cfg.bridge.role == stream_bridge_server::config::Role::Receiver,
        "bot bridge role must be receiver"
    );
    cfg.bridge.validate()?;
    let mut shreds = ShredReceiver::bind(cfg.shred).await?;
    // Keep the bridge future in this task: exiting drops it and its child tasks.
    let bridge = stream_bridge_server::run(cfg.bridge);
    tokio::pin!(bridge);
    let bot = async {
        let x_token = cfg.yellowstone.x_token_env.map(std::env::var).transpose()?;
        let tls = if cfg.yellowstone.endpoint.starts_with("https://") {
            let mut tls = ClientTlsConfig::new().with_native_roots();
            if let Some(domain) = cfg.yellowstone.tls_domain {
                tls = tls.domain_name(domain);
            }
            if let Some(path) = cfg.yellowstone.ca_file {
                tls = tls.ca_certificate(
                    yellowstone_grpc_proto::tonic::transport::Certificate::from_pem(std::fs::read(
                        path,
                    )?),
                );
            }
            Some(tls)
        } else {
            anyhow::ensure!(
                cfg.yellowstone.tls_domain.is_none() && cfg.yellowstone.ca_file.is_none(),
                "TLS options require https endpoint"
            );
            None
        };
        let mut request = SubscribeRequest::default();
        request
            .slots
            .insert("bot-slots".into(), SubscribeRequestFilterSlots::default());
        if !cfg.yellowstone.transaction_accounts.is_empty() {
            request.transactions.insert(
                "bot-transactions".into(),
                SubscribeRequestFilterTransactions {
                    vote: Some(false),
                    failed: Some(false),
                    account_include: cfg.yellowstone.transaction_accounts,
                    ..Default::default()
                },
            );
        }
        // Retry initial connection while the embedded receiver is starting.
        let mut attempts = 0;
        let mut grpc = loop {
            match YellowstoneSubscription::connect(
                cfg.yellowstone.endpoint.clone(),
                x_token.clone(),
                tls.clone(),
                request.clone(),
            )
            .await
            {
                Ok(subscription) => break subscription,
                Err(error) if attempts < 10 => {
                    attempts += 1;
                    tracing::warn!(%error, "waiting for gRPC source");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        };
        loop {
            tokio::select! {
                packet = shreds.recv() => {
                    let packet = packet.context("shred input closed")??;
                    // Feed packet.data into your Bot's shred/entry parser here.
                    println!("shred: peer={} bytes={} channel_drops={}", packet.peer, packet.data.len(), shreds.dropped_packets());
                }
                update = grpc.next_update() => {
                    let update = update?.context("Yellowstone stream ended; reconnect and resubscribe")?;
                    match update.update_oneof {
                        Some(subscribe_update::UpdateOneof::Slot(slot)) => println!("slot: {}", slot.slot),
                        Some(subscribe_update::UpdateOneof::Transaction(tx)) => {
                            println!("transaction: slot={} signature={:?}", tx.slot, tx.transaction.map(|t| t.signature));
                            // Invoke your strategy with the decoded transaction here.
                        }
                        _ => {}
                    }
                }
            }
        }
    };
    tokio::select! {
        result = &mut bridge => result,
        result = bot => result,
        result = tokio::signal::ctrl_c() => { result?; Ok(()) },
    }
}
