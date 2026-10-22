//! One upstream Yellowstone subscription, bounded per-client fanout and admission limits.
mod access;
mod config;
pub mod dashboard;
mod dispatch;
mod filters;
mod io;
mod metrics;
mod state;
#[cfg(test)]
mod tests;
mod upstream;
pub use config::SharedConfig;

use crate::{config::Grpc, tunnel::TunnelPool};
use anyhow::{Context, Result};
use futures::Stream;
use state::{Hub, Mailbox, Registry};
use std::{
    pin::Pin,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use yellowstone_grpc_proto::{
    prelude::*,
    prost::Message,
    tonic::{self, Request, Response, Status, Streaming},
};

type Updates = Pin<Box<dyn Stream<Item = Result<SubscribeUpdate, Status>> + Send>>;
#[derive(Clone)]
struct Service {
    hub: Hub,
    cfg: Arc<SharedConfig>,
    metrics: Arc<metrics::Metrics>,
    queue_bytes: Arc<Semaphore>,
    initial_timeout: Duration,
}
struct Pending {
    user: String,
    hub: Hub,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.hub.lock().unwrap().release_reservation(&self.user);
    }
}
struct Session {
    id: u64,
    hub: Hub,
    reader: JoinHandle<()>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.reader.abort();
        self.hub
            .lock()
            .unwrap()
            .remove(self.id, Status::cancelled("client disconnected"));
    }
}
impl Service {
    fn authenticate<T>(&self, req: &Request<T>) -> Result<String, Status> {
        let peer = req
            .remote_addr()
            .ok_or_else(|| Status::unauthenticated("peer address required"))?
            .ip();
        self.hub
            .lock()
            .unwrap()
            .access
            .authenticate(req.metadata(), peer)
            .inspect_err(|_| {
                self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            })
    }
}
#[tonic::async_trait]
impl geyser_server::Geyser for Service {
    type SubscribeStream = Updates;
    async fn subscribe(
        &self,
        req: Request<Streaming<SubscribeRequest>>,
    ) -> Result<Response<Updates>, Status> {
        let user = self.authenticate(&req)?;
        self.hub.lock().unwrap().reserve(&user).inspect_err(|_| {
            self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
        })?;
        let pending = Pending {
            user: user.clone(),
            hub: self.hub.clone(),
        };
        let peer = req
            .remote_addr()
            .ok_or_else(|| Status::unauthenticated("peer address required"))?
            .ip();
        let metadata = req.metadata().clone();
        let mut input = req.into_inner();
        let mut request = timeout(self.initial_timeout, input.message())
            .await
            .map_err(|_| Status::deadline_exceeded("initial subscription required"))??
            .ok_or_else(|| Status::invalid_argument("initial subscription required"))?;
        let initial_ping = request.ping.take();
        let mailbox = Mailbox::new(&self.cfg, self.queue_bytes.clone());
        let id = {
            let mut registry = self.hub.lock().unwrap();
            // Re-check after the asynchronous first-message wait: a key/IP can be
            // revoked while this authenticated request holds a pending slot.
            let current_user = registry.access.authenticate(&metadata, peer)?;
            if current_user != user {
                return Err(Status::unauthenticated("access policy changed; reconnect"));
            }
            registry
                .register_peer(user, peer, request, mailbox.clone())
                .inspect_err(|_| {
                    self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
                })?
        };
        drop(pending);
        let mut closed = mailbox.closed.subscribe();
        let (hub, output, metrics) = (self.hub.clone(), mailbox.clone(), self.metrics.clone());
        let reader = tokio::spawn(async move {
            if let Some(ping) = initial_ping {
                send_pong(&hub, id, &output, ping.id);
            }
            loop {
                tokio::select! {
                    biased;
                    _ = closed.changed() => break,
                    request = input.message() => {
                        match request {
                            Ok(Some(mut request)) => {
                                // Yellowstone ping-only requests leave existing filters intact.
                                if let Some(ping) = request.ping.take() {
                                    send_pong(&hub, id, &output, ping.id);
                                    continue;
                                }
                                let result = hub.lock().unwrap().update(id, request);
                                if let Err(status) = result {
                                    metrics.rejected.fetch_add(1, Ordering::Relaxed);
                                    hub.lock().unwrap().remove(id, status);
                                    break;
                                }
                            }
                            // A clean half-close is valid: keep sending the established subscription.
                            Ok(None) => break,
                            Err(_) => { hub.lock().unwrap().remove(id, Status::cancelled("client request stream failed")); break; }
                        }
                    }
                }
            }
        });
        let session = Session {
            id,
            hub: self.hub.clone(),
            reader,
        };
        let stream = async_stream::stream! {
            let _session = session;
            loop {
                let update = mailbox.recv().await;
                let failed = update.is_err();
                yield update;
                if failed { break; }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }
    async fn ping(&self, req: Request<PingRequest>) -> Result<Response<PongResponse>, Status> {
        self.authenticate(&req)?;
        Ok(Response::new(PongResponse {
            count: req.into_inner().count,
        }))
    }
    async fn get_version(
        &self,
        req: Request<GetVersionRequest>,
    ) -> Result<Response<GetVersionResponse>, Status> {
        self.authenticate(&req)?;
        Ok(Response::new(GetVersionResponse {
            version: format!("sol-stream-bridge/{} shared", env!("CARGO_PKG_VERSION")),
        }))
    }
    type SubscribeDeshredStream =
        Pin<Box<dyn Stream<Item = Result<SubscribeUpdateDeshred, Status>> + Send>>;
    async fn subscribe_deshred(
        &self,
        _: Request<Streaming<SubscribeDeshredRequest>>,
    ) -> Result<Response<Self::SubscribeDeshredStream>, Status> {
        Err(unsupported())
    }
    type SubscribeGossipStream =
        Pin<Box<dyn Stream<Item = Result<SubscribeUpdateGossip, Status>> + Send>>;
    async fn subscribe_gossip(
        &self,
        _: Request<SubscribeGossipRequest>,
    ) -> Result<Response<Self::SubscribeGossipStream>, Status> {
        Err(unsupported())
    }
    async fn subscribe_replay_info(
        &self,
        _: Request<SubscribeReplayInfoRequest>,
    ) -> Result<Response<SubscribeReplayInfoResponse>, Status> {
        Err(unsupported())
    }
    async fn get_latest_blockhash(
        &self,
        _: Request<GetLatestBlockhashRequest>,
    ) -> Result<Response<GetLatestBlockhashResponse>, Status> {
        Err(unsupported())
    }
    async fn get_block_height(
        &self,
        _: Request<GetBlockHeightRequest>,
    ) -> Result<Response<GetBlockHeightResponse>, Status> {
        Err(unsupported())
    }
    async fn get_slot(
        &self,
        _: Request<GetSlotRequest>,
    ) -> Result<Response<GetSlotResponse>, Status> {
        Err(unsupported())
    }
    async fn is_blockhash_valid(
        &self,
        _: Request<IsBlockhashValidRequest>,
    ) -> Result<Response<IsBlockhashValidResponse>, Status> {
        Err(unsupported())
    }
}
fn unsupported() -> Status {
    Status::unimplemented(
        "shared mode exposes Subscribe, Ping and GetVersion only; use passthrough for other RPCs",
    )
}
fn send_pong(hub: &Hub, id: u64, output: &Mailbox, ping: i32) {
    let update = SubscribeUpdate {
        update_oneof: Some(subscribe_update::UpdateOneof::Pong(SubscribeUpdatePong {
            id: ping,
        })),
        ..Default::default()
    };
    let encoded_bytes = update.encoded_len();
    if matches!(
        output.send(Arc::new(update), Arc::from([]), encoded_bytes),
        state::Delivery::Full
    ) {
        let mut registry = hub.lock().unwrap();
        let status = Status::resource_exhausted("client queue limit reached");
        if output.fail(status.clone()) {
            registry
                .metrics
                .slow_clients
                .fetch_add(1, Ordering::Relaxed);
        }
        registry.remove(id, status);
    }
}

pub(crate) fn validate_upstream(g: &Grpc) -> Result<()> {
    if let Some(cfg) = &g.shared {
        upstream::builder(g, cfg)?;
        dashboard::validate(g, cfg)?;
    }
    Ok(())
}

pub(crate) async fn serve(g: Grpc, clients: TcpListener, pool: TunnelPool) -> Result<()> {
    let cfg = g.shared.clone().context("shared configuration required")?;
    cfg.validate()?;
    let metrics = Arc::new(metrics::Metrics::default());
    let (hub, changes) = Registry::new(cfg.clone(), metrics.clone());
    let managed = dashboard::load_state(&g, &cfg)?;
    hub.lock()
        .unwrap()
        .apply_access(access::AccessPolicy::new(managed)?);
    let service = Service {
        hub: hub.clone(),
        cfg: Arc::new(cfg.clone()),
        metrics: metrics.clone(),
        queue_bytes: Arc::new(Semaphore::new(cfg.total_queue_bytes)),
        initial_timeout: Duration::from_millis(g.connect_timeout_ms),
    };
    let mut server = tonic::transport::Server::builder()
        .tcp_nodelay(true)
        .http2_keepalive_interval(Some(Duration::from_secs(30)))
        .max_concurrent_streams(Some(cfg.max_subscriptions as u32));
    if cfg.client_tls {
        let cert = std::fs::read(
            g.cert_file
                .as_ref()
                .context("client TLS requires grpc.cert_file")?,
        )?;
        let key = std::fs::read(
            g.key_file
                .as_ref()
                .context("client TLS requires grpc.key_file")?,
        )?;
        server = server.tls_config(
            tonic::transport::ServerTlsConfig::new()
                .identity(tonic::transport::Identity::from_pem(cert, key)),
        )?;
    }
    let mut workers = JoinSet::<Result<()>>::new();
    workers.spawn(upstream::run(
        g.clone(),
        cfg.clone(),
        pool,
        hub.clone(),
        changes,
        metrics.clone(),
    ));
    let stats = metrics.clone();
    workers.spawn(async move {
        metrics::log(stats).await;
        Ok(())
    });
    if let Some(addr) = cfg.metrics_listen {
        workers.spawn(metrics::serve(
            TcpListener::bind(addr).await?,
            metrics.clone(),
        ));
    }
    if let Some(admin) = &cfg.dashboard {
        workers.spawn(dashboard::serve(
            admin.clone(),
            g.clone(),
            hub.clone(),
            metrics.clone(),
        ));
    }
    let slots = Arc::new(Semaphore::new(g.max_connections));
    let incoming: Pin<
        Box<dyn Stream<Item = std::io::Result<io::Tracked<tokio::net::TcpStream>>> + Send>,
    > = Box::pin(async_stream::try_stream! {
        loop {
            let (tcp, peer) = clients.accept().await?;
            if !hub.lock().unwrap().access.allows(peer.ip()) { metrics.rejected.fetch_add(1, Ordering::Relaxed); continue; }
            let Ok(permit) = slots.clone().try_acquire_owned() else { metrics.rejected.fetch_add(1, Ordering::Relaxed); continue; };
            tcp.set_nodelay(true)?;
            yield io::Tracked::new(tcp, metrics.clone(), false, permit);
        }
    });
    let server = server
        .add_service(
            geyser_server::GeyserServer::new(service)
                .max_decoding_message_size(cfg.max_request_bytes)
                .max_encoding_message_size(cfg.max_response_bytes),
        )
        .serve_with_incoming(incoming);
    tokio::select! {
        result = server => result?,
        result = workers.join_next() => { result.context("shared worker missing")???; anyhow::bail!("shared worker exited"); }
    }
    Ok(())
}
