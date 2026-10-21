//! Bot-facing raw UDP reception. A bounded channel prevents unbounded memory use.
use anyhow::{ensure, Result};
use serde::Deserialize;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::{sync::mpsc, task::JoinHandle};

/// Normalize IPv4-mapped IPv6 addresses for exact source-IP matching.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShredInput {
    pub listen: SocketAddr,
    #[serde(default)]
    pub allowed_sources: Vec<IpAddr>,
    #[serde(default = "buffer_size")]
    pub receive_buffer: usize,
    #[serde(default = "capacity")]
    pub channel_capacity: usize,
}
fn buffer_size() -> usize {
    8 * 1024 * 1024
}
fn capacity() -> usize {
    4096
}

#[derive(Debug)]
pub struct ShredPacket {
    /// The complete UDP payload, not a decoded transaction or entry.
    pub data: Vec<u8>,
    pub peer: SocketAddr,
    pub received_at: Instant,
}

pub struct ShredReceiver {
    rx: mpsc::Receiver<Result<ShredPacket>>,
    worker: JoinHandle<()>,
    local_addr: SocketAddr,
    dropped: Arc<AtomicU64>,
}
impl ShredReceiver {
    /// Bind before returning so startup errors are observable by the bot.
    pub async fn bind(config: ShredInput) -> Result<Self> {
        ensure!(
            config.channel_capacity > 0,
            "channel_capacity must be positive"
        );
        ensure!(config.receive_buffer > 0, "receive_buffer must be positive");
        let socket = bind_udp(config.listen, config.receive_buffer)?;
        let local_addr = socket.local_addr()?;
        let (tx, rx) = mpsc::channel(config.channel_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let counter = dropped.clone();
        let allowed: Vec<_> = config
            .allowed_sources
            .into_iter()
            .map(canonical_ip)
            .collect();
        let worker = tokio::spawn(async move {
            let mut buf = vec![0; 65535];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((n, peer)) => {
                        if !allowed.is_empty() && !allowed.contains(&canonical_ip(peer.ip())) {
                            continue;
                        }
                        // Reserve before copying so overload does not allocate discarded packets.
                        match tx.try_reserve() {
                            Ok(permit) => permit.send(Ok(ShredPacket {
                                data: buf[..n].to_vec(),
                                peer,
                                received_at: Instant::now(),
                            })),
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                counter.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error.into())).await;
                        break;
                    }
                }
            }
        });
        Ok(Self {
            rx,
            worker,
            local_addr,
            dropped,
        })
    }
    /// Cancellation safe: use in tokio::select! alongside gRPC updates.
    pub async fn recv(&mut self) -> Option<Result<ShredPacket>> {
        self.rx.recv().await
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    /// Newest packets dropped due to a full Bot channel (excludes kernel/network loss).
    pub fn dropped_packets(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
impl Drop for ShredReceiver {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

#[cfg(feature = "yellowstone")]
pub mod yellowstone {
    use anyhow::Result;
    use futures::{Sink, SinkExt, Stream, StreamExt};
    use prelude::{
        subscribe_update::UpdateOneof, SubscribeRequest, SubscribeRequestPing, SubscribeUpdate,
    };
    use std::{pin::Pin, time::Duration};
    pub use yellowstone_grpc_client::ClientTlsConfig;
    pub use yellowstone_grpc_proto::prelude;

    type Requests =
        Pin<Box<dyn Sink<SubscribeRequest, Error = futures::channel::mpsc::SendError> + Send>>;
    type Updates = Pin<Box<dyn Stream<Item = Result<SubscribeUpdate>> + Send>>;

    pub struct YellowstoneSubscription {
        requests: Requests,
        updates: Updates,
    }
    impl YellowstoneSubscription {
        /// Connect to the receiver's gRPC listen endpoint, not its tunnel port.
        /// TLS here authenticates the upstream Yellowstone service, not the bridge tunnel.
        pub async fn connect(
            endpoint: String,
            x_token: Option<String>,
            tls: Option<ClientTlsConfig>,
            request: SubscribeRequest,
        ) -> Result<Self> {
            let mut builder =
                yellowstone_grpc_client::GeyserGrpcClient::build_from_shared(endpoint)?
                    .x_token(x_token)?
                    .connect_timeout(Duration::from_secs(10))
                    .max_decoding_message_size(64 * 1024 * 1024);
            if let Some(tls) = tls {
                builder = builder.tls_config(tls)?;
            }
            let mut client = builder.connect().await?;
            let (requests, updates) = client.subscribe_with_request(Some(request)).await?;
            Ok(Self {
                requests: Box::pin(requests),
                updates: Box::pin(updates.map(|r| r.map_err(Into::into))),
            })
        }
        /// Read typed Yellowstone messages. Errors/EOF are returned to the Bot;
        /// reconnect and resubscribe explicitly (no silent replay or gap hiding).
        pub async fn next_update(&mut self) -> Result<Option<SubscribeUpdate>> {
            let Some(update) = self.updates.next().await else {
                return Ok(None);
            };
            let update = update?;
            if matches!(update.update_oneof, Some(UpdateOneof::Ping(_))) {
                // The client's request sink is an unbounded in-process queue. Enqueue
                // synchronously so select! cancellation cannot lose a consumed update.
                self.requests.as_mut().start_send(SubscribeRequest {
                    ping: Some(SubscribeRequestPing { id: 1 }),
                    ..Default::default()
                })?;
            }
            Ok(Some(update))
        }
        /// A replacement subscription request; callers provide the complete filters.
        pub async fn update_filters(&mut self, request: SubscribeRequest) -> Result<()> {
            self.requests.send(request).await?;
            Ok(())
        }
    }
}

fn bind_udp(listen: SocketAddr, receive_buffer: usize) -> Result<tokio::net::UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(listen),
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_recv_buffer_size(receive_buffer)?;
    socket.set_nonblocking(true)?;
    socket.bind(&listen.into())?;
    Ok(tokio::net::UdpSocket::from_std(socket.into())?)
}
