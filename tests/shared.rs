use futures::StreamExt;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use stream_bridge_server::{config::Grpc, tunnel};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use yellowstone_grpc_proto::{
    prelude::*,
    tonic::{
        self,
        transport::{Channel, Endpoint},
        Code, Request, Response, Status, Streaming,
    },
};

struct Mock {
    sessions: mpsc::UnboundedSender<mpsc::Sender<Result<SubscribeUpdate, Status>>>,
    requests: mpsc::UnboundedSender<SubscribeRequest>,
}
#[tonic::async_trait]
impl geyser_server::Geyser for Mock {
    type SubscribeStream = ReceiverStream<Result<SubscribeUpdate, Status>>;
    async fn subscribe(
        &self,
        req: Request<Streaming<SubscribeRequest>>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        assert_eq!(req.metadata().get("x-token").unwrap(), "provider-secret");
        assert!(req.metadata().get("x-api-key").is_none());
        let mut input = req.into_inner();
        let requests = self.requests.clone();
        tokio::spawn(async move {
            while let Ok(Some(r)) = input.message().await {
                if requests.send(r).is_err() {
                    break;
                }
            }
        });
        let (tx, rx) = mpsc::channel(8);
        self.sessions.send(tx).unwrap();
        Ok(Response::new(ReceiverStream::new(rx)))
    }
    type SubscribeDeshredStream = ReceiverStream<Result<SubscribeUpdateDeshred, Status>>;
    async fn subscribe_deshred(
        &self,
        _: Request<Streaming<SubscribeDeshredRequest>>,
    ) -> Result<Response<Self::SubscribeDeshredStream>, Status> {
        Err(Status::unimplemented("unused"))
    }
    type SubscribeGossipStream = ReceiverStream<Result<SubscribeUpdateGossip, Status>>;
    async fn subscribe_gossip(
        &self,
        _: Request<SubscribeGossipRequest>,
    ) -> Result<Response<Self::SubscribeGossipStream>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn subscribe_replay_info(
        &self,
        _: Request<SubscribeReplayInfoRequest>,
    ) -> Result<Response<SubscribeReplayInfoResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn ping(&self, _: Request<PingRequest>) -> Result<Response<PongResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn get_latest_blockhash(
        &self,
        _: Request<GetLatestBlockhashRequest>,
    ) -> Result<Response<GetLatestBlockhashResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn get_block_height(
        &self,
        _: Request<GetBlockHeightRequest>,
    ) -> Result<Response<GetBlockHeightResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn get_slot(
        &self,
        _: Request<GetSlotRequest>,
    ) -> Result<Response<GetSlotResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn is_blockhash_valid(
        &self,
        _: Request<IsBlockhashValidRequest>,
    ) -> Result<Response<IsBlockhashValidResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
    async fn get_version(
        &self,
        _: Request<GetVersionRequest>,
    ) -> Result<Response<GetVersionResponse>, Status> {
        Err(Status::unimplemented("unused"))
    }
}
fn port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}
fn config(public: SocketAddr, upstream: SocketAddr) -> Grpc {
    std::env::set_var(
        "SHARED_TEST_TUNNEL_TOKEN",
        "0123456789abcdef0123456789abcdef",
    );
    std::env::set_var("SHARED_TEST_PROVIDER_TOKEN", "provider-secret");
    toml::from_str(&format!(
        r#"
    tunnel_address="{}"
    listen="{public}"
    upstream="{upstream}"
    token_env="SHARED_TEST_TUNNEL_TOKEN"
    allowed_sources=["127.0.0.1"]
    insecure=true
    idle_tunnels=2
    reconnect_ms=20
    api_keys={{ alice="alice-secret", bob="bob-secret", carol="carol-secret" }}
    [shared]
    upstream_x_token_env="SHARED_TEST_PROVIDER_TOKEN"
    max_subscriptions_per_key=1
    max_filters=2
    "#,
        port()
    ))
    .unwrap()
}
async fn channel(addr: SocketAddr) -> Channel {
    loop {
        if let Ok(channel) = Endpoint::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect_timeout(Duration::from_millis(100))
            .connect()
            .await
        {
            return channel;
        }
        sleep(Duration::from_millis(10)).await;
    }
}
fn tx(name: &str, key: u8) -> SubscribeRequest {
    SubscribeRequest {
        transactions: [(
            name.into(),
            SubscribeRequestFilterTransactions {
                account_include: vec![bs58::encode([key; 32]).into_string()],
                ..Default::default()
            },
        )]
        .into(),
        ..Default::default()
    }
}
async fn subscribe(
    channel: Channel,
    key: &str,
    request: SubscribeRequest,
) -> Result<(mpsc::Sender<SubscribeRequest>, Streaming<SubscribeUpdate>), Status> {
    let (tx, rx) = mpsc::channel(8);
    tx.send(request).await.unwrap();
    let mut req = Request::new(ReceiverStream::new(rx));
    req.metadata_mut().insert("x-token", key.parse().unwrap());
    let output = geyser_client::GeyserClient::new(channel)
        .subscribe(req)
        .await?
        .into_inner();
    Ok((tx, output))
}
async fn union(
    rx: &mut mpsc::UnboundedReceiver<SubscribeRequest>,
    count: usize,
) -> SubscribeRequest {
    loop {
        let r = rx.recv().await.unwrap();
        if r.transactions.len() == count {
            return r;
        }
    }
}
#[tokio::test]
async fn real_shared_tunnel_auth_fanout_limits_and_reconnect() {
    exercise_shared(false).await;
}
#[tokio::test]
async fn shared_tls_provider_over_plaintext_tunnel() {
    exercise_shared(true).await;
}
async fn exercise_shared(provider_tls: bool) {
    timeout(Duration::from_secs(15), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let public = port();
        let mut cfg = config(public, listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut server = tonic::transport::Server::builder();
        if provider_tls {
            let cert = rcgen::generate_simple_self_signed(vec!["provider.test".into()]).unwrap();
            let path = dir.path().join("provider.pem");
            std::fs::write(&path, cert.cert.pem()).unwrap();
            let shared = cfg.shared.as_mut().unwrap();
            shared.upstream_endpoint = "https://provider.test".into();
            shared.upstream_ca_file = Some(path.to_str().unwrap().into());
            server = server
                .tls_config(tonic::transport::ServerTlsConfig::new().identity(
                    tonic::transport::Identity::from_pem(
                        cert.cert.pem(),
                        cert.key_pair.serialize_pem(),
                    ),
                ))
                .unwrap();
        }
        tunnel::validate_tls(&cfg, false).unwrap();
        let connects = Arc::new(AtomicUsize::new(0));
        let counter = connects.clone();
        let (sessions, mut session_rx) = mpsc::unbounded_channel();
        let (requests, mut request_rx) = mpsc::unbounded_channel();
        let incoming = TcpListenerStream::new(listener).inspect(move |r| {
            if r.is_ok() {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            server
                .add_service(geyser_server::GeyserServer::new(Mock {
                    sessions,
                    requests,
                }))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let g = cfg.clone();
        workers.spawn(async move {
            tunnel::receiver(g).await.unwrap();
        });
        let client = channel(public).await;
        assert_eq!(
            subscribe(client.clone(), "bad", tx("bad", 1))
                .await
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );
        assert_eq!(connects.load(Ordering::Relaxed), 0);
        let mut source = cfg.clone();
        source.shared = None;
        source.max_connections = 1;
        source.api_keys = Default::default();
        workers.spawn(async move {
            tunnel::source(source).await.unwrap();
        });
        let (a_tx, mut a) = subscribe(client.clone(), "alice-secret", tx("alice", 1))
            .await
            .unwrap();
        let output = session_rx.recv().await.unwrap();
        union(&mut request_rx, 1).await;
        let (b_tx, mut b) = subscribe(channel(public).await, "bob-secret", tx("bob", 2))
            .await
            .unwrap();
        let merged = union(&mut request_rx, 2).await;
        let (_c_tx, mut c) = subscribe(channel(public).await, "carol-secret", tx("carol", 1))
            .await
            .unwrap();
        assert_eq!(
            subscribe(client.clone(), "alice-secret", tx("too-many", 1))
                .await
                .unwrap_err()
                .code(),
            Code::ResourceExhausted
        );
        assert_eq!(connects.load(Ordering::Relaxed), 1);
        assert!(session_rx.try_recv().is_err());
        let labels = merged.transactions.keys().cloned().collect();
        output
            .send(Ok(SubscribeUpdate {
                filters: labels,
                update_oneof: Some(subscribe_update::UpdateOneof::Transaction(
                    SubscribeUpdateTransaction {
                        slot: 42,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(a.message().await.unwrap().unwrap().filters, vec!["alice"]);
        assert_eq!(b.message().await.unwrap().unwrap().filters, vec!["bob"]);
        assert_eq!(c.message().await.unwrap().unwrap().filters, vec!["carol"]);
        // Request half-close must keep delivery alive.
        drop(a_tx);
        // Bad replacement closes bob alone and retains alice/carol's shared filter.
        b_tx.send(SubscribeRequest {
            transactions: [("unfiltered".into(), Default::default())].into(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            b.message().await.unwrap_err().code(),
            Code::PermissionDenied
        );
        let remaining = union(&mut request_rx, 1).await;
        output
            .send(Ok(SubscribeUpdate {
                filters: remaining.transactions.keys().cloned().collect(),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(a.message().await.unwrap().unwrap().filters, vec!["alice"]);
        assert_eq!(c.message().await.unwrap().unwrap().filters, vec!["carol"]);
        output
            .send(Err(Status::unavailable("provider closed")))
            .await
            .unwrap();
        assert_eq!(a.message().await.unwrap_err().code(), Code::Unavailable);
        assert_eq!(c.message().await.unwrap_err().code(), Code::Unavailable);
        let (_tx, mut again) = subscribe(client, "alice-secret", tx("again", 1))
            .await
            .unwrap();
        let output2 = session_rx.recv().await.unwrap();
        let r = union(&mut request_rx, 1).await;
        output2
            .send(Ok(SubscribeUpdate {
                filters: r.transactions.keys().cloned().collect(),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(
            again.message().await.unwrap().unwrap().filters,
            vec!["again"]
        );
        assert_eq!(connects.load(Ordering::Relaxed), 2);
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn ip_allowlist_and_optional_keys_are_independent() {
    timeout(Duration::from_secs(5), async {
        let public = port();
        let mut cfg = config(public, port());
        cfg.api_keys.0.values_mut().for_each(String::clear);
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            tunnel::receiver(cfg).await.unwrap();
        });
        let mut client = geyser_client::GeyserClient::new(channel(public).await);
        assert_eq!(
            client
                .ping(PingRequest { count: 7 })
                .await
                .unwrap()
                .into_inner()
                .count,
            7
        );
        let public = port();
        let mut cfg = config(public, port());
        cfg.allowed_sources = vec!["192.0.2.1".parse().unwrap()];
        workers.spawn(async move {
            tunnel::receiver(cfg).await.unwrap();
        });
        loop {
            if TcpStream::connect(public).await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        let endpoint = Endpoint::from_shared(format!("http://{public}"))
            .unwrap()
            .connect_timeout(Duration::from_millis(200));
        if let Ok(ch) = endpoint.connect().await {
            let mut req = Request::new(PingRequest { count: 7 });
            req.metadata_mut()
                .insert("x-token", "alice-secret".parse().unwrap());
            assert!(geyser_client::GeyserClient::new(ch)
                .ping(req)
                .await
                .is_err());
        }
    })
    .await
    .unwrap();
}
