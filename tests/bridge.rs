use std::{
    net::{SocketAddr, TcpListener as StdListener},
    time::Duration,
};
use stream_bridge_server::{
    config::{Config, Grpc, Role, Shred},
    tunnel, udp,
};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    time::{sleep, timeout},
};
use tonic_health::pb::{health_client::HealthClient, HealthCheckRequest};

fn port() -> SocketAddr {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}
fn udp_port() -> SocketAddr {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}
fn grpc_config(tunnel: SocketAddr, public: SocketAddr, upstream: SocketAddr) -> Grpc {
    // All tests use one immutable value for this variable.
    std::env::set_var("BRIDGE_TEST_TOKEN", "0123456789abcdef0123456789abcdef");
    Grpc {
        tunnel_address: tunnel.to_string(),
        listen: Some(public),
        upstream: Some(upstream.to_string()),
        token_env: "BRIDGE_TEST_TOKEN".into(),
        allowed_sources: vec!["127.0.0.1".parse().unwrap()],
        insecure: true,
        cert_file: None,
        key_file: None,
        ca_file: None,
        server_name: None,
        idle_tunnels: 2,
        max_connections: 16,
        reconnect_ms: 20,
        connect_timeout_ms: 1000,
    }
}
async fn listening(addr: SocketAddr) {
    timeout(Duration::from_secs(5), async {
        loop {
            if TcpStream::connect(addr).await.is_ok() {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn health(public: SocketAddr) -> HealthClient<tonic::transport::Channel> {
    timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(channel) = tonic::transport::Endpoint::from_shared(format!("http://{public}"))
                .unwrap()
                .connect()
                .await
            {
                let mut c = HealthClient::new(channel);
                if c.check(HealthCheckRequest {
                    service: String::new(),
                })
                .await
                .is_ok()
                {
                    return c;
                }
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn tls_grpc_unary_streaming_and_source_reconnect() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let (mut reporter, service) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(upstream))
            .await
            .unwrap();
    });
    let (tunnel_addr, public) = (port(), port());
    let mut g = grpc_config(tunnel_addr, public, upstream_addr);
    let dir = tempfile::tempdir().unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();
    g.insecure = false;
    g.cert_file = Some(cert_path.to_str().unwrap().into());
    g.key_file = Some(key_path.to_str().unwrap().into());
    g.ca_file = g.cert_file.clone();
    g.server_name = Some("localhost".into());
    let receiver = tokio::spawn(tunnel::receiver(g.clone()));
    listening(tunnel_addr).await;
    let udp_output = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let udp_listen = udp_port();
    let source_config = Config {
        role: Role::Source,
        grpc: Some(g.clone()),
        shred: vec![Shred {
            name: "concurrent".into(),
            listen: udp_listen,
            targets: vec![udp_output.local_addr().unwrap()],
            allowed_sources: vec![],
            receive_buffer: 65536,
        }],
    };
    let source = tokio::spawn(stream_bridge_server::run(source_config));
    let mut client = health(public).await;
    let mut stream = client
        .watch(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stream.message().await.unwrap().unwrap().status, 1);
    reporter
        .set_service_status("", tonic_health::ServingStatus::NotServing)
        .await;
    timeout(Duration::from_secs(3), async {
        loop {
            if stream.message().await.unwrap().unwrap().status == 2 {
                break;
            }
        }
    })
    .await
    .unwrap();
    // Verify UDP forwarding while the same process keeps a gRPC stream open.
    let udp_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp_sender
        .send_to(b"parallel-shred", udp_listen)
        .await
        .unwrap();
    let mut payload = [0; 128];
    let n = timeout(Duration::from_secs(2), udp_output.recv(&mut payload))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&payload[..n], b"parallel-shred");
    drop(stream);
    drop(client);
    source.abort();
    let _ = source.await;
    let source = tokio::spawn(tunnel::source(g));
    let mut client = health(public).await;
    assert_eq!(
        client
            .check(HealthCheckRequest {
                service: String::new()
            })
            .await
            .unwrap()
            .into_inner()
            .status,
        2
    );
    source.abort();
    receiver.abort();
    server.abort();
}

#[tokio::test]
async fn udp_preserves_datagrams_and_fans_out() {
    let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen = udp_port();
    let task = tokio::spawn(udp::run(Shred {
        name: "test".into(),
        listen,
        targets: vec![a.local_addr().unwrap(), b.local_addr().unwrap()],
        allowed_sources: vec!["127.0.0.1".parse().unwrap()],
        receive_buffer: 1024 * 1024,
    }));
    let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    // Retry a probe until the UDP listener has bound; consume it at both destinations.
    timeout(Duration::from_secs(3), async {
        let mut buf = [0; 16];
        loop {
            sender.send_to(b"ready", listen).await.unwrap();
            if timeout(Duration::from_millis(30), a.recv(&mut buf))
                .await
                .is_ok()
            {
                break;
            }
        }
        timeout(Duration::from_secs(1), b.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
    })
    .await
    .unwrap();
    for payload in [
        vec![],
        vec![7; 1232],
        (0..8192).map(|n| (n % 256) as u8).collect(),
    ] {
        sender.send_to(&payload, listen).await.unwrap();
        for output in [&a, &b] {
            let mut buf = vec![0; 65535];
            let n = timeout(Duration::from_secs(2), output.recv(&mut buf))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&buf[..n], payload.as_slice());
        }
    }
    task.abort();
}

#[tokio::test]
async fn udp_source_allowlist_denies_other_ips() {
    let output = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen = udp_port();
    let task = tokio::spawn(udp::run(Shred {
        name: "deny".into(),
        listen,
        targets: vec![output.local_addr().unwrap()],
        allowed_sources: vec!["192.0.2.1".parse().unwrap()],
        receive_buffer: 65536,
    }));
    sleep(Duration::from_millis(30)).await;
    let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for _ in 0..5 {
        sender.send_to(b"denied", listen).await.unwrap();
        sleep(Duration::from_millis(10)).await;
    }
    assert!(
        timeout(Duration::from_millis(100), output.recv(&mut [0; 64]))
            .await
            .is_err()
    );
    assert!(!task.is_finished());
    task.abort();
}

#[test]
fn rejects_unknown_keys_and_invalid_routes() {
    assert!(toml::from_str::<Config>("role='source'\nshreds=[]").is_err());
    let cfg = Config {
        role: Role::Source,
        grpc: None,
        shred: vec![],
    };
    assert!(cfg.validate().is_err());
    let cfg = Config {
        shred: vec![Shred {
            name: "loop".into(),
            listen: "0.0.0.0:18001".parse().unwrap(),
            targets: vec!["127.0.0.1:18001".parse().unwrap()],
            allowed_sources: vec![],
            receive_buffer: 65536,
        }],
        ..cfg
    };
    assert!(cfg.validate().is_err());
}

#[tokio::test]
async fn invalid_tunnel_token_is_rejected() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (address, public) = (port(), port());
    let g = grpc_config(address, public, port());
    let task = tokio::spawn(tunnel::receiver(g));
    listening(address).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(b"SGBR0001").await.unwrap();
    stream.write_all(&[0; 32]).await.unwrap();
    let result = timeout(Duration::from_secs(2), stream.read_u8())
        .await
        .unwrap();
    assert!(result.is_err());
    task.abort();
}

#[tokio::test]
async fn grpc_denied_clients_never_consume_a_tunnel() {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for allowed in [vec![], vec!["192.0.2.1".parse().unwrap()]] {
        let (address, public) = (port(), port());
        let mut g = grpc_config(address, public, port());
        g.allowed_sources = allowed;
        let task = tokio::spawn(tunnel::receiver(g));
        listening(address).await;
        // Authenticate a source and verify that a rejected client never sends OPEN to it.
        let mut source = TcpStream::connect(address).await.unwrap();
        source.write_all(b"SGBR0001").await.unwrap();
        source
            .write_all(&Sha256::digest(b"0123456789abcdef0123456789abcdef"))
            .await
            .unwrap();
        assert_eq!(source.read_u8().await.unwrap(), 1);
        let mut client = TcpStream::connect(public).await.unwrap();
        let read = timeout(Duration::from_secs(1), client.read(&mut [0; 1]))
            .await
            .unwrap();
        assert!(
            matches!(read, Ok(0)) || read.is_err(),
            "denied client must be closed"
        );
        assert!(
            timeout(Duration::from_millis(150), source.read_u8())
                .await
                .is_err(),
            "denied client must not open a tunnel"
        );
        task.abort();
    }
}

#[test]
fn grpc_allowlist_defaults_to_deny_and_matches_ip_addresses() {
    let base = r#"role = "receiver"
[grpc]
tunnel_address = "127.0.0.1:19000"
listen = "127.0.0.1:10000"
token_env = "BRIDGE_TEST_TOKEN"
insecure = true
"#;
    let cfg: Config = toml::from_str(base).unwrap();
    assert!(!cfg
        .grpc
        .unwrap()
        .allows_client("127.0.0.1".parse().unwrap()));
    let cfg: Config = toml::from_str(&format!(
        "{base}allowed_sources = ['127.0.0.1', '2001:db8::1']"
    ))
    .unwrap();
    let g = cfg.grpc.unwrap();
    for ip in ["127.0.0.1", "::ffff:127.0.0.1", "2001:db8::1"] {
        assert!(g.allows_client(ip.parse().unwrap()));
    }
    for ip in ["127.0.0.2", "::1", "2001:db8::2"] {
        assert!(!g.allows_client(ip.parse().unwrap()));
    }
    assert!(toml::from_str::<Config>(&format!("{base}allowed_sources = ['not-an-ip']")).is_err());
}
