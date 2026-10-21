use crate::config::Shred;
use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::{SocketAddr, UdpSocket as OutputSocket},
    time::Duration,
};
use stream_bridge_client::canonical_ip;
use tokio::{net::UdpSocket, time::interval};
use tracing::info;

pub(crate) fn bind(listen: SocketAddr, receive_buffer: usize) -> Result<UdpSocket> {
    let socket = Socket::new(
        Domain::for_address(listen),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    socket.set_recv_buffer_size(receive_buffer)?;
    socket.set_nonblocking(true)?;
    socket.bind(&listen.into())?;
    info!(%listen, actual_buffer = socket.recv_buffer_size()?, "UDP socket bound");
    Ok(UdpSocket::from_std(socket.into())?)
}

pub async fn run(cfg: Shred) -> Result<()> {
    let input = bind(cfg.listen, cfg.receive_buffer)?;
    let mut outputs = Vec::with_capacity(cfg.targets.len());
    for target in &cfg.targets {
        let bind: SocketAddr = if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        }
        .parse()?;
        // Direct nonblocking sends avoid readiness waits and queues in the hot path.
        let output = OutputSocket::bind(bind)?;
        output.set_nonblocking(true)?;
        output.connect(target)?;
        outputs.push(output);
    }
    info!(route = %cfg.name, listen = %cfg.listen, targets = ?cfg.targets, "UDP route ready");
    let allowed: Vec<_> = cfg.allowed_sources.into_iter().map(canonical_ip).collect();
    let mut buf = vec![0; 65535];
    let (mut received, mut forwarded, mut denied, mut errors) = (0u64, 0u64, 0u64, 0u64);
    let mut stats = interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            r = input.recv_from(&mut buf) => {
                let (n, peer) = r?;
                received += 1;
                if !allowed.is_empty() && !allowed.contains(&canonical_ip(peer.ip())) {
                    denied += 1;
                    continue;
                }
                // Keep each original UDP datagram intact, including zero-length packets.
                for output in &outputs {
                    match output.send(&buf[..n]) {
                        Ok(_) => forwarded += 1,
                        // Includes WouldBlock: drop only this destination's packet.
                        Err(_) => errors += 1,
                    }
                }
            }
            _ = stats.tick() => info!(route = %cfg.name, received, forwarded, denied, errors, "UDP counters"),
        }
    }
}
