//! A pool of authenticated reverse TCP connections. Each gRPC TCP connection
//! owns one tunnel: HTTP/2 framing, protobuf data, TLS and metadata are opaque.
use crate::config::Grpc;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex, Semaphore},
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName},
    },
    TlsAcceptor, TlsConnector,
};
use tracing::{info, warn};

trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}
type Pipe = Box<dyn Transport>;
const MAGIC: &[u8; 8] = b"SGBR0001";
const AUTH_OK: u8 = 1;
const OPEN: u8 = 2;
const READY: u8 = 3;

fn duration(g: &Grpc) -> Duration {
    Duration::from_millis(g.connect_timeout_ms)
}
fn tune(s: &TcpStream) -> Result<()> {
    s.set_nodelay(true)?;
    socket2::SockRef::from(s).set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(30))
            .with_interval(Duration::from_secs(10)),
    )?;
    Ok(())
}
fn connector(g: &Grpc) -> Result<Option<TlsConnector>> {
    if g.insecure {
        return Ok(None);
    }
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = &g.ca_file {
        for cert in CertificateDer::pem_file_iter(path)? {
            roots.add(cert?)?;
        }
    }
    let c = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(TlsConnector::from(Arc::new(c))))
}
fn acceptor(g: &Grpc) -> Result<Option<TlsAcceptor>> {
    if g.insecure {
        return Ok(None);
    }
    let certs = CertificateDer::pem_file_iter(g.cert_file.as_ref().context("cert_file required")?)?
        .collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(g.key_file.as_ref().context("key_file required")?)?;
    let c = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Some(TlsAcceptor::from(Arc::new(c))))
}
pub fn validate_tls(g: &Grpc, source: bool) -> Result<()> {
    if source {
        connector(g)?;
        if !g.insecure {
            ServerName::try_from(g.server_name.clone().context("server_name required")?)?;
        }
    } else {
        acceptor(g)?;
    }
    Ok(())
}
async fn auth(pipe: &mut Pipe, token: &[u8; 32], server: bool) -> Result<()> {
    if server {
        let mut magic = [0; 8];
        pipe.read_exact(&mut magic).await?;
        let mut received = [0; 32];
        pipe.read_exact(&mut received).await?;
        if &magic != MAGIC || !bool::from(token.ct_eq(&received)) {
            bail!("invalid tunnel credentials or version");
        }
        pipe.write_u8(AUTH_OK).await?;
        pipe.flush().await?;
    } else {
        pipe.write_all(MAGIC).await?;
        pipe.write_all(token).await?;
        pipe.flush().await?;
        if pipe.read_u8().await? != AUTH_OK {
            bail!("tunnel authentication rejected");
        }
    }
    Ok(())
}
async fn source_pipe(g: &Grpc, tls: &Option<TlsConnector>, token: &[u8; 32]) -> Result<Pipe> {
    let tcp = TcpStream::connect(&g.tunnel_address).await?;
    tune(&tcp)?;
    let mut pipe: Pipe = if let Some(tls) = tls {
        Box::new(
            tls.connect(
                ServerName::try_from(g.server_name.clone().context("server_name required")?)?,
                tcp,
            )
            .await?,
        )
    } else {
        Box::new(tcp)
    };
    auth(&mut pipe, token, false).await?;
    Ok(pipe)
}
pub async fn source(g: Grpc) -> Result<()> {
    let tls = connector(&g)?;
    let token: [u8; 32] = Sha256::digest(g.token()?.as_bytes()).into();
    let slots = Arc::new(Semaphore::new(g.max_connections));
    let mut workers = JoinSet::new();
    info!(receiver = %g.tunnel_address, upstream = ?g.upstream, idle_tunnels = g.idle_tunnels, "starting reverse gRPC tunnel pool");
    for index in 0..g.idle_tunnels {
        let (g, tls, slots) = (g.clone(), tls.clone(), slots.clone());
        workers.spawn(async move {
            let mut relays = JoinSet::new();
            loop {
                while relays.try_join_next().is_some() {}
                let result = async {
                    let mut pipe = timeout(duration(&g), source_pipe(&g, &tls, &token)).await??;
                    // Idle tunnel lifetime is unlimited; TCP keepalive detects dead peers.
                    if pipe.read_u8().await? != OPEN {
                        bail!("invalid tunnel OPEN");
                    }
                    let permit = slots
                        .clone()
                        .try_acquire_owned()
                        .context("upstream connection limit reached")?;
                    let upstream_addr = g.upstream.clone().context("upstream required")?;
                    let connect_timeout = duration(&g);
                    relays.spawn(async move {
                        let _permit = permit;
                        let result = async {
                            let mut upstream =
                                timeout(connect_timeout, TcpStream::connect(upstream_addr))
                                    .await??;
                            tune(&upstream)?;
                            pipe.write_u8(READY).await?;
                            pipe.flush().await?;
                            tokio::io::copy_bidirectional(&mut pipe, &mut upstream).await?;
                            Ok::<_, anyhow::Error>(())
                        }
                        .await;
                        if let Err(error) = result {
                            warn!(%error, "upstream relay ended");
                        }
                    });
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                if let Err(error) = result {
                    warn!(worker = index, %error, "source tunnel unavailable; retrying");
                    sleep(Duration::from_millis(g.reconnect_ms)).await;
                }
            }
        });
    }
    workers.join_next().await.context("no source workers")??;
    bail!("source worker exited unexpectedly")
}

pub async fn receiver(g: Grpc) -> Result<()> {
    let tls = acceptor(&g)?;
    let token: [u8; 32] = Sha256::digest(g.token()?.as_bytes()).into();
    let tunnels = TcpListener::bind(&g.tunnel_address).await?;
    let clients = TcpListener::bind(g.listen.context("listen required")?).await?;
    let (tx, rx) = mpsc::channel::<Pipe>(g.idle_tunnels);
    let rx = Arc::new(Mutex::new(rx));
    let handshake_slots = Arc::new(Semaphore::new(g.idle_tunnels + 32));
    let client_slots = Arc::new(Semaphore::new(g.max_connections));
    let mut tasks = JoinSet::new();
    info!(tunnel = %tunnels.local_addr()?, grpc = %clients.local_addr()?, "receiver ready");
    loop {
        tokio::select! {
            accepted = tunnels.accept() => {
                let (tcp, peer) = accepted?;
                let Ok(permit) = handshake_slots.clone().try_acquire_owned() else { continue; };
                tune(&tcp)?;
                let (tls, tx) = (tls.clone(), tx.clone());
                let d = duration(&g);
                tasks.spawn(async move {
                    let _permit = permit;
                    let result = timeout(d, async {
                        let mut pipe: Pipe = if let Some(tls) = tls { Box::new(tls.accept(tcp).await?) } else { Box::new(tcp) };
                        auth(&mut pipe, &token, true).await?;
                        Ok::<_, anyhow::Error>(pipe)
                    }).await;
                    match result {
                        Ok(Ok(pipe)) => { let _ = tx.send(pipe).await; }
                        other => warn!(%peer, error = ?other.err().map(|e| e.to_string()).unwrap_or_else(|| "authentication/TLS failed".into()), "tunnel rejected"),
                    }
                });
            }
            accepted = clients.accept() => {
                let (mut client, peer) = accepted?;
                // Reject before consuming a connection slot or opening any upstream tunnel.
                if !g.allows_client(peer.ip()) {
                    tracing::debug!(%peer, "gRPC client denied by IP allowlist");
                    continue;
                }
                let Ok(permit) = client_slots.clone().try_acquire_owned() else { continue; };
                tune(&client)?;
                let rx = rx.clone();
                let d = duration(&g);
                let open_timeout = Duration::from_millis(g.tunnel_open_timeout_ms).min(d / 2);
                tasks.spawn(async move {
                    let _permit = permit;
                    // Discard stale idle tunnels, but never replay application data.
                    let selected = timeout(d, async {
                        loop {
                            let mut pipe = rx.lock().await.recv().await.context("tunnel pool closed")?;
                            let ready = timeout(open_timeout, async {
                                pipe.write_u8(OPEN).await?;
                                pipe.flush().await?;
                                if pipe.read_u8().await? != READY { bail!("invalid tunnel READY"); }
                                Ok::<_, anyhow::Error>(())
                            }).await;
                            if matches!(ready, Ok(Ok(()))) {
                                return Ok::<_, anyhow::Error>(pipe);
                            }
                        }
                    }).await;
                    match selected {
                        Ok(Ok(mut pipe)) => {
                            if let Err(error) = tokio::io::copy_bidirectional(&mut client, &mut pipe).await { warn!(%peer, %error, "client relay ended"); }
                        }
                        _ => warn!(%peer, "no ready source tunnel before timeout"),
                    }
                });
            }
            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(Err(error)) = result { return Err(error.into()); }
            }
        }
    }
}
