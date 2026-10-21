use anyhow::{Context, Result};
use serde::Deserialize;
use stream_bridge_client::{
    yellowstone::{prelude::*, ClientTlsConfig, YellowstoneSubscription},
    ShredInput, ShredReceiver,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    endpoint: String,
    x_token_env: Option<String>,
    /// Optional upstream certificate domain when connecting through a local endpoint.
    tls_domain: Option<String>,
    ca_file: Option<String>,
    #[serde(default)]
    transaction_accounts: Vec<String>,
}

pub async fn receive(shred: ShredInput, yellowstone: SubscriptionConfig) -> Result<()> {
    let mut shreds = ShredReceiver::bind(shred).await?;
    tokio::try_join!(
        receive_shreds(&mut shreds),
        receive_yellowstone(yellowstone)
    )?;
    Ok(())
}

async fn receive_shreds(shreds: &mut ShredReceiver) -> Result<()> {
    loop {
        let packet = shreds.recv().await.context("shred input closed")??;
        // Feed packet.data into your Bot's shred/entry parser here.
        println!(
            "shred: peer={} bytes={} channel_drops={}",
            packet.peer,
            packet.data.len(),
            shreds.dropped_packets()
        );
    }
}

async fn receive_yellowstone(cfg: SubscriptionConfig) -> Result<()> {
    let x_token = cfg.x_token_env.map(std::env::var).transpose()?;
    let tls = if cfg.endpoint.starts_with("https://") {
        let mut tls = ClientTlsConfig::new().with_native_roots();
        if let Some(domain) = cfg.tls_domain {
            tls = tls.domain_name(domain);
        }
        if let Some(path) = cfg.ca_file {
            tls = tls.ca_certificate(
                yellowstone_grpc_proto::tonic::transport::Certificate::from_pem(std::fs::read(
                    path,
                )?),
            );
        }
        Some(tls)
    } else {
        anyhow::ensure!(
            cfg.tls_domain.is_none() && cfg.ca_file.is_none(),
            "TLS options require https endpoint"
        );
        None
    };
    let mut request = SubscribeRequest::default();
    request
        .slots
        .insert("bot-slots".into(), SubscribeRequestFilterSlots::default());
    if !cfg.transaction_accounts.is_empty() {
        request.transactions.insert(
            "bot-transactions".into(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                account_include: cfg.transaction_accounts,
                ..Default::default()
            },
        );
    }
    // Initial retries run alongside UDP reception.
    let mut attempts = 0;
    let mut grpc = loop {
        match YellowstoneSubscription::connect(
            cfg.endpoint.clone(),
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
        let update = grpc
            .next_update()
            .await?
            .context("Yellowstone stream ended; reconnect and resubscribe")?;
        match update.update_oneof {
            Some(subscribe_update::UpdateOneof::Slot(slot)) => println!("slot: {}", slot.slot),
            Some(subscribe_update::UpdateOneof::Transaction(tx)) => {
                println!(
                    "transaction: slot={} signature={:?}",
                    tx.slot,
                    tx.transaction.map(|t| t.signature)
                );
                // Invoke your strategy with the decoded transaction here.
            }
            _ => {}
        }
    }
}
