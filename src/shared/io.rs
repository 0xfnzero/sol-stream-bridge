use super::metrics::Metrics;
use std::{
    pin::Pin,
    sync::{atomic::Ordering, Arc},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::OwnedSemaphorePermit,
};
use yellowstone_grpc_proto::tonic::transport::server::{Connected, TcpConnectInfo};

pub struct Tracked<T> {
    inner: T,
    metrics: Arc<Metrics>,
    upstream: bool,
    _permit: OwnedSemaphorePermit,
}
impl<T> Tracked<T> {
    pub fn new(
        inner: T,
        metrics: Arc<Metrics>,
        upstream: bool,
        permit: OwnedSemaphorePermit,
    ) -> Self {
        if upstream {
            metrics.upstream_connections.fetch_add(1, Ordering::Relaxed);
        } else {
            metrics.connections.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            inner,
            metrics,
            upstream,
            _permit: permit,
        }
    }
}
impl<T> Drop for Tracked<T> {
    fn drop(&mut self) {
        if self.upstream {
            self.metrics
                .upstream_connections
                .fetch_sub(1, Ordering::Relaxed);
        } else {
            self.metrics.connections.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for Tracked<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for Tracked<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
impl Connected for Tracked<TcpStream> {
    type ConnectInfo = TcpConnectInfo;
    fn connect_info(&self) -> Self::ConnectInfo {
        self.inner.connect_info()
    }
}
