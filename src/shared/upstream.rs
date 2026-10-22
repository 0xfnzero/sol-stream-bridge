use super::{filters, io::Tracked, metrics::Metrics, state::Hub, SharedConfig};
use crate::{
    config::Grpc,
    tunnel::{open_tunnel, TunnelPool},
};
use anyhow::{Context, Result};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch, Semaphore},
    time::{sleep, timeout},
};
use yellowstone_grpc_client::{
    ClientTlsConfig, GeyserGrpcBuilder, GeyserGrpcClient, InterceptorXToken,
};
use yellowstone_grpc_proto::{
    prelude::*,
    tonic::{transport::Certificate, Status},
};

struct ActiveStream(Arc<Metrics>);
impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.0.upstream_streams.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) fn builder(g: &Grpc, cfg: &SharedConfig) -> Result<GeyserGrpcBuilder> {
    let token = cfg
        .upstream_x_token_env
        .as_ref()
        .filter(|name| !name.is_empty())
        .map(std::env::var)
        .transpose()
        .context("missing shared upstream token environment variable")?;
    let mut builder = GeyserGrpcClient::build_from_shared(cfg.upstream_endpoint.clone())?
        .x_token(token)?
        .connect_timeout(Duration::from_millis(g.connect_timeout_ms));
    if cfg.upstream_endpoint.starts_with("https://") {
        let mut tls = ClientTlsConfig::new().with_native_roots();
        if let Some(domain) = &cfg.upstream_tls_domain {
            tls = tls.domain_name(domain);
        }
        if let Some(path) = &cfg.upstream_ca_file {
            tls = tls.ca_certificate(Certificate::from_pem(std::fs::read(path)?));
        }
        builder = builder.tls_config(tls)?;
    }
    Ok(builder)
}

pub async fn run(
    g: Grpc,
    cfg: SharedConfig,
    pool: TunnelPool,
    hub: Hub,
    mut changes: watch::Receiver<Arc<SubscribeRequest>>,
    metrics: Arc<Metrics>,
) -> Result<()> {
    let builder = builder(&g, &cfg)?;
    let dispatcher = hub.lock().unwrap().dispatcher.clone();
    // Hold the permit for the receiver-side tunnel socket lifetime. Also cap
    // source max_connections at 1 for strict provider limits during TCP teardown.
    let one_connection = Arc::new(Semaphore::new(1));
    loop {
        while filters::filter_count(&changes.borrow_and_update()) == 0 {
            if changes.changed().await.is_err() {
                return Ok(());
            }
        }
        let result: Result<()> = async {
            let (pool, connection_metrics, slots) = (pool.clone(), metrics.clone(), one_connection.clone());
            let wait = Duration::from_millis(g.connect_timeout_ms);
            let open_wait = Duration::from_millis(g.tunnel_open_timeout_ms).min(wait / 2);
            let channel = builder.endpoint.connect_with_connector(tower::service_fn(move |_| {
                let (pool, metrics, slots) = (pool.clone(), connection_metrics.clone(), slots.clone());
                async move {
                    let permit = slots.acquire_owned().await.map_err(std::io::Error::other)?;
                    let pipe = open_tunnel(pool, wait, open_wait).await.map_err(std::io::Error::other)?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(Tracked::new(pipe, metrics, true, permit)))
                }
            })).await?;
            let mut client = geyser_client::GeyserClient::with_interceptor(channel, InterceptorXToken {
                x_token: builder.x_token.clone(), x_request_snapshot: false,
            }).max_decoding_message_size(cfg.max_response_bytes);
            let mut requests = changes.clone();
            let (ping_tx, mut ping_rx) = mpsc::channel::<()>(1);
            let outgoing = async_stream::stream! {
                let initial = (**requests.borrow_and_update()).clone();
                yield initial;
                loop {
                    tokio::select! {
                        biased;
                        ping = ping_rx.recv() => {
                            if ping.is_none() { break; }
                            yield SubscribeRequest { ping: Some(SubscribeRequestPing { id: 1 }), ..Default::default() };
                        }
                        changed = requests.changed() => {
                            if changed.is_err() { break; }
                            let latest = (**requests.borrow_and_update()).clone();
                            yield latest;
                        }
                    }
                }
            };
            let mut stream = timeout(wait, client.subscribe(outgoing)).await??.into_inner();
            metrics.upstream_streams.fetch_add(1, Ordering::Relaxed);
            let _active = ActiveStream(metrics.clone());
            loop {
                tokio::select! {
                    changed = changes.changed() => {
                        if changed.is_err() || filters::filter_count(&changes.borrow_and_update()) == 0 { return Ok(()); }
                    }
                    update = stream.message() => {
                        let update = update?.context("upstream subscription ended")?;
                        match &update.update_oneof {
                            Some(subscribe_update::UpdateOneof::Ping(_)) => { let _ = ping_tx.try_send(()); }
                            Some(subscribe_update::UpdateOneof::Pong(_)) => {}
                            _ => {
                                let slow = dispatcher.dispatch(update);
                                if !slow.is_empty() { hub.lock().unwrap().remove_slow(slow); }
                            },
                        }
                    }
                }
            }
        }.await;
        if result.is_err() {
            metrics.upstream_failures.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                "shared upstream unavailable; closing downstream subscriptions without replay"
            );
            hub.lock().unwrap().fail_all(Status::unavailable(
                "upstream disconnected; reconnect and resubscribe (no historical replay)",
            ));
            sleep(Duration::from_millis(g.reconnect_ms)).await;
        }
    }
}
