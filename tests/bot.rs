use stream_bridge_server::bot::{ShredInput, ShredReceiver};
use tokio::{
    net::UdpSocket,
    time::{sleep, timeout, Duration},
};

#[tokio::test]
async fn bot_udp_matches_all_ipv4_mapped_spellings() {
    for allowed in ["127.0.0.1", "::ffff:127.0.0.1", "::ffff:7f00:1"] {
        let mut receiver = ShredReceiver::bind(ShredInput {
            listen: "[::]:0".parse().unwrap(),
            allowed_sources: vec![allowed.parse().unwrap()],
            receive_buffer: 65536,
            channel_capacity: 1,
        })
        .await
        .unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(
                b"mapped",
                (std::net::Ipv4Addr::LOCALHOST, receiver.local_addr().port()),
            )
            .await
            .unwrap();
        let packet = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(packet.data, b"mapped");
        let denied = UdpSocket::bind("[::1]:0").await.unwrap();
        denied
            .send_to(
                b"denied",
                (std::net::Ipv6Addr::LOCALHOST, receiver.local_addr().port()),
            )
            .await
            .unwrap();
        assert!(timeout(Duration::from_millis(50), receiver.recv())
            .await
            .is_err());
    }
}

#[tokio::test]
async fn bot_receives_bytes_and_reports_queue_overflow() {
    let mut receiver = ShredReceiver::bind(ShredInput {
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_sources: vec!["127.0.0.1".parse().unwrap()],
        receive_buffer: 65536,
        channel_capacity: 1,
    })
    .await
    .unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for payload in [vec![], vec![0, 255, 2]] {
        sender
            .send_to(&payload, receiver.local_addr())
            .await
            .unwrap();
        let packet = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(packet.data, payload);
        assert_eq!(packet.peer, sender.local_addr().unwrap());
    }
    for _ in 0..32 {
        sender
            .send_to(b"overflow", receiver.local_addr())
            .await
            .unwrap();
    }
    timeout(Duration::from_secs(1), async {
        while receiver.dropped_packets() == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn bot_shred_allowlist_denies_other_sources() {
    let mut receiver = ShredReceiver::bind(ShredInput {
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_sources: vec!["192.0.2.1".parse().unwrap()],
        receive_buffer: 65536,
        channel_capacity: 1,
    })
    .await
    .unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender
        .send_to(b"denied", receiver.local_addr())
        .await
        .unwrap();
    assert!(timeout(Duration::from_millis(100), receiver.recv())
        .await
        .is_err());
}

#[cfg(feature = "yellowstone")]
mod grpc {
    use super::*;
    use stream_bridge_server::bot::yellowstone::{prelude::*, YellowstoneSubscription};
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use yellowstone_grpc_proto::tonic::{self, Request, Response, Status, Streaming};
    struct Mock;
    #[tonic::async_trait]
    impl geyser_server::Geyser for Mock {
        type SubscribeStream = ReceiverStream<Result<SubscribeUpdate, Status>>;
        async fn subscribe(
            &self,
            req: Request<Streaming<SubscribeRequest>>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            if req.metadata().get("x-token").and_then(|v| v.to_str().ok()) != Some("test-token") {
                return Err(Status::unauthenticated("missing token"));
            }
            let mut input = req.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let first = input.message().await.unwrap().unwrap();
                assert!(first.slots.contains_key("slots"));
                tx.send(Ok(SubscribeUpdate {
                    update_oneof: Some(subscribe_update::UpdateOneof::Ping(SubscribeUpdatePing {})),
                    ..Default::default()
                }))
                .await
                .unwrap();
                assert!(input.message().await.unwrap().unwrap().ping.is_some());
                tx.send(Ok(SubscribeUpdate {
                    update_oneof: Some(subscribe_update::UpdateOneof::Slot(SubscribeUpdateSlot {
                        slot: 42,
                        ..Default::default()
                    })),
                    ..Default::default()
                }))
                .await
                .unwrap();
                assert!(input
                    .message()
                    .await
                    .unwrap()
                    .unwrap()
                    .transactions
                    .contains_key("tx"));
                tx.send(Ok(SubscribeUpdate {
                    update_oneof: Some(subscribe_update::UpdateOneof::Transaction(
                        SubscribeUpdateTransaction {
                            slot: 43,
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }))
                .await
                .unwrap();
            });
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
    #[tokio::test]
    async fn typed_yellowstone_updates_ping_filters_and_eof() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(geyser_server::GeyserServer::new(Mock))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        timeout(Duration::from_secs(5),async {
            let mut request=SubscribeRequest::default();request.slots.insert("slots".into(),Default::default());
            let mut client=YellowstoneSubscription::connect(format!("http://{address}"),Some("test-token".into()),None,request).await.unwrap();
            assert!(matches!(client.next_update().await.unwrap().unwrap().update_oneof,Some(subscribe_update::UpdateOneof::Ping(_))));
            let update=client.next_update().await.unwrap().unwrap();
            assert!(matches!(update.update_oneof,Some(subscribe_update::UpdateOneof::Slot(slot)) if slot.slot==42));
            let mut request=SubscribeRequest::default();request.transactions.insert("tx".into(),Default::default());
            client.update_filters(request).await.unwrap();
            let update=client.next_update().await.unwrap().unwrap();
            assert!(matches!(update.update_oneof,Some(subscribe_update::UpdateOneof::Transaction(tx)) if tx.slot==43));
            assert!(client.next_update().await.unwrap().is_none());
        }).await.unwrap();
        server.abort();
    }
}
