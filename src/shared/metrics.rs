use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
    task::JoinSet,
    time::{interval, timeout, Duration},
};

#[derive(Default)]
pub struct Metrics {
    pub connections: AtomicUsize,
    pub subscriptions: AtomicUsize,
    pub filters: AtomicUsize,
    pub accounts: AtomicUsize,
    pub upstream_connections: AtomicUsize,
    pub upstream_streams: AtomicUsize,
    pub rejected: AtomicU64,
    pub slow_clients: AtomicU64,
    pub upstream_failures: AtomicU64,
    pub updates: AtomicU64,
    pub delivered: AtomicU64,
}
impl Metrics {
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (name, value, kind) in [
            (
                "connections",
                self.connections.load(Ordering::Relaxed) as u64,
                "gauge",
            ),
            (
                "subscriptions",
                self.subscriptions.load(Ordering::Relaxed) as u64,
                "gauge",
            ),
            (
                "upstream_connections",
                self.upstream_connections.load(Ordering::Relaxed) as u64,
                "gauge",
            ),
            (
                "upstream_streams",
                self.upstream_streams.load(Ordering::Relaxed) as u64,
                "gauge",
            ),
            (
                "filters",
                self.filters.load(Ordering::Relaxed) as u64,
                "gauge",
            ),
            (
                "accounts",
                self.accounts.load(Ordering::Relaxed) as u64,
                "gauge",
            ),
            (
                "rejected_total",
                self.rejected.load(Ordering::Relaxed),
                "counter",
            ),
            (
                "slow_clients_total",
                self.slow_clients.load(Ordering::Relaxed),
                "counter",
            ),
            (
                "upstream_failures_total",
                self.upstream_failures.load(Ordering::Relaxed),
                "counter",
            ),
            (
                "updates_total",
                self.updates.load(Ordering::Relaxed),
                "counter",
            ),
            (
                "deliveries_total",
                self.delivered.load(Ordering::Relaxed),
                "counter",
            ),
        ] {
            out.push_str(&format!(
                "# TYPE stream_bridge_{name} {kind}\nstream_bridge_{name} {value}\n"
            ));
        }
        out
    }
}
pub async fn log(metrics: Arc<Metrics>) {
    let mut tick = interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        tracing::info!(
            connections = metrics.connections.load(Ordering::Relaxed),
            subscriptions = metrics.subscriptions.load(Ordering::Relaxed),
            upstream_connections = metrics.upstream_connections.load(Ordering::Relaxed),
            upstream_streams = metrics.upstream_streams.load(Ordering::Relaxed),
            filters = metrics.filters.load(Ordering::Relaxed),
            accounts = metrics.accounts.load(Ordering::Relaxed),
            rejected = metrics.rejected.load(Ordering::Relaxed),
            slow_clients = metrics.slow_clients.load(Ordering::Relaxed),
            "shared gRPC statistics"
        );
    }
}
pub async fn serve(listener: TcpListener, metrics: Arc<Metrics>) -> anyhow::Result<()> {
    let slots = Arc::new(Semaphore::new(8));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (mut socket, _) = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let metrics = metrics.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let _ = timeout(Duration::from_secs(2), async {
                        let mut head = Vec::with_capacity(128);
                        while head.len() < 1024 && !head.ends_with(b"\r\n\r\n") { head.push(socket.read_u8().await?); }
                        let (status, body) = if head.starts_with(b"GET /metrics HTTP/1.") { ("200 OK", metrics.text()) } else { ("404 Not Found", String::new()) };
                        socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await
                    }).await;
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}
