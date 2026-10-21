use super::{config::UserLimits, filters, metrics::Metrics, state::*, *};
use yellowstone_grpc_proto::tonic::Code;

fn tx(name: &str, account: u8) -> SubscribeRequest {
    SubscribeRequest {
        transactions: [(
            name.into(),
            SubscribeRequestFilterTransactions {
                account_include: vec![bs58::encode([account; 32]).into_string()],
                ..Default::default()
            },
        )]
        .into(),
        ..Default::default()
    }
}
fn mailbox(cfg: &SharedConfig) -> Arc<Mailbox> {
    Mailbox::new(cfg, Arc::new(Semaphore::new(cfg.total_queue_bytes)))
}
fn update(label: String) -> SubscribeUpdate {
    SubscribeUpdate {
        filters: vec![label],
        update_oneof: Some(subscribe_update::UpdateOneof::Transaction(
            SubscribeUpdateTransaction {
                slot: 42,
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

#[tokio::test]
async fn shared_routes_restore_names_and_reject_excess_without_mutation() {
    let cfg = SharedConfig {
        max_filters: 2,
        ..Default::default()
    };
    let (hub, changes) = Registry::new(cfg.clone(), Arc::default());
    let (a, b, c) = (mailbox(&cfg), mailbox(&cfg), mailbox(&cfg));
    let aid = hub
        .lock()
        .unwrap()
        .register("alice".into(), tx("alice", 1), a.clone())
        .unwrap();
    let bid = hub
        .lock()
        .unwrap()
        .register("bob".into(), tx("bob", 2), b.clone())
        .unwrap();
    hub.lock()
        .unwrap()
        .register("carol".into(), tx("carol", 1), c.clone())
        .unwrap();
    let before = changes.borrow().clone();
    assert_eq!(before.transactions.len(), 2);
    assert_eq!(
        hub.lock()
            .unwrap()
            .register("extra".into(), tx("extra", 3), mailbox(&cfg))
            .unwrap_err()
            .code(),
        Code::ResourceExhausted
    );
    let mut too_many = tx("old", 2);
    too_many.transactions.extend(tx("new", 3).transactions);
    assert!(hub.lock().unwrap().update(bid, too_many).is_err());
    assert_eq!(*changes.borrow().clone(), *before);
    let label = before
        .transactions
        .iter()
        .find(|(_, f)| f.account_include == tx("x", 1).transactions["x"].account_include)
        .unwrap()
        .0
        .clone();
    hub.lock().unwrap().dispatch(update(label.clone()));
    assert_eq!(a.recv().await.unwrap().filters, vec!["alice"]);
    assert_eq!(c.recv().await.unwrap().filters, vec!["carol"]);
    assert!(timeout(Duration::from_millis(20), b.recv()).await.is_err());
    // Alice switches to bob's existing filter; old in-flight labels must not reach alice.
    hub.lock().unwrap().update(aid, tx("alice-new", 2)).unwrap();
    hub.lock().unwrap().dispatch(update(label));
    assert_eq!(c.recv().await.unwrap().filters, vec!["carol"]);
    assert!(timeout(Duration::from_millis(20), a.recv()).await.is_err());
    hub.lock().unwrap().dispatch(update("unknown".into()));
    assert!(timeout(Duration::from_millis(20), b.recv()).await.is_err());
    hub.lock()
        .unwrap()
        .fail_all(Status::unavailable("upstream ended"));
    assert_eq!(a.recv().await.unwrap_err().code(), Code::Unavailable);
    assert_eq!(b.recv().await.unwrap_err().code(), Code::Unavailable);
    assert_eq!(filters::filter_count(&changes.borrow()), 0);
}

#[tokio::test]
async fn slow_user_does_not_block_other_users_and_releases_memory() {
    let cfg = SharedConfig {
        client_queue_capacity: 1,
        ..Default::default()
    };
    let metrics = Arc::new(Metrics::default());
    let (hub, changes) = Registry::new(cfg.clone(), metrics.clone());
    let global = Arc::new(Semaphore::new(cfg.total_queue_bytes));
    let a = Mailbox::new(&cfg, global.clone());
    let b = Mailbox::new(&cfg, global.clone());
    hub.lock()
        .unwrap()
        .register("slow".into(), tx("a", 1), a.clone())
        .unwrap();
    hub.lock()
        .unwrap()
        .register("fast".into(), tx("b", 1), b.clone())
        .unwrap();
    let label = changes.borrow().transactions.keys().next().unwrap().clone();
    hub.lock().unwrap().dispatch(update(label.clone()));
    assert_eq!(b.recv().await.unwrap().filters, vec!["b"]);
    hub.lock().unwrap().dispatch(update(label));
    assert_eq!(b.recv().await.unwrap().filters, vec!["b"]);
    assert_eq!(a.recv().await.unwrap_err().code(), Code::ResourceExhausted);
    assert_eq!(metrics.subscriptions.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.slow_clients.load(Ordering::Relaxed), 1);
    assert_eq!(global.available_permits(), cfg.total_queue_bytes);
}

#[test]
fn per_user_limits_span_multiple_subscriptions_and_account_permission() {
    let mut cfg = SharedConfig::default();
    cfg.users.insert(
        "alice".into(),
        UserLimits {
            max_filters: Some(1),
            max_accounts: Some(1),
            max_subscriptions: Some(2),
            allow_accounts: Some(true),
        },
    );
    let (hub, _) = Registry::new(cfg.clone(), Arc::default());
    let account = SubscribeRequest {
        accounts: [(
            "account".into(),
            SubscribeRequestFilterAccounts {
                account: vec![bs58::encode([1; 32]).into_string()],
                ..Default::default()
            },
        )]
        .into(),
        ..Default::default()
    };
    assert_eq!(
        hub.lock()
            .unwrap()
            .register("bob".into(), account.clone(), mailbox(&cfg))
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let id = hub
        .lock()
        .unwrap()
        .register("alice".into(), account, mailbox(&cfg))
        .unwrap();
    assert_eq!(
        hub.lock()
            .unwrap()
            .register("alice".into(), tx("tx", 1), mailbox(&cfg))
            .unwrap_err()
            .code(),
        Code::ResourceExhausted
    );
    hub.lock()
        .unwrap()
        .update(id, tx("replacement", 2))
        .unwrap();
    hub.lock()
        .unwrap()
        .register("bob".into(), tx("b", 2), mailbox(&cfg))
        .unwrap();
    hub.lock()
        .unwrap()
        .register("bob".into(), tx("b2", 2), mailbox(&cfg))
        .unwrap();
    assert_eq!(
        hub.lock()
            .unwrap()
            .register("bob".into(), tx("b3", 2), mailbox(&cfg))
            .unwrap_err()
            .code(),
        Code::ResourceExhausted
    );
}

#[test]
fn reject_unbounded_invalid_and_unsupported_subscriptions() {
    let cfg = SharedConfig::default();
    let mut request = tx("tx", 1);
    assert!(filters::validate(&request, &cfg).is_ok());
    request
        .transactions
        .get_mut("tx")
        .unwrap()
        .account_include
        .clear();
    assert_eq!(
        filters::validate(&request, &cfg).unwrap_err().code(),
        Code::PermissionDenied
    );
    request.transactions.get_mut("tx").unwrap().account_exclude =
        vec![bs58::encode([1; 32]).into_string()];
    assert!(filters::validate(&request, &cfg).is_err());
    request = tx("tx", 1);
    request.transactions.get_mut("tx").unwrap().account_include = vec!["bad".into()];
    assert!(filters::validate(&request, &cfg).is_err());
    request = tx("tx", 1);
    request.from_slot = Some(1);
    assert!(filters::validate(&request, &cfg).is_err());
    request = tx("tx", 1);
    request
        .accounts_data_slice
        .push(SubscribeRequestAccountsDataSlice {
            offset: 0,
            length: 1,
        });
    assert!(filters::validate(&request, &cfg).is_err());
    request = tx("tx", 1);
    request.commitment = Some(CommitmentLevel::Finalized as i32);
    assert!(filters::validate(&request, &cfg).is_err());
    let blocks = SubscribeRequest {
        blocks: [(
            "block".into(),
            SubscribeRequestFilterBlocks {
                account_include: vec![bs58::encode([1; 32]).into_string()],
                include_accounts: Some(true),
                ..Default::default()
            },
        )]
        .into(),
        ..Default::default()
    };
    assert_eq!(
        filters::validate(
            &blocks,
            &SharedConfig {
                allow_blocks: true,
                ..cfg
            }
        )
        .unwrap_err()
        .code(),
        Code::PermissionDenied
    );
}

#[test]
fn pending_requests_cannot_bypass_user_subscription_quota() {
    let cfg = SharedConfig {
        max_subscriptions: 2,
        max_subscriptions_per_key: 1,
        ..Default::default()
    };
    let (hub, _) = Registry::new(cfg.clone(), Arc::default());
    let mut registry = hub.lock().unwrap();
    registry.reserve("alice").unwrap();
    assert_eq!(
        registry.reserve("alice").unwrap_err().code(),
        Code::ResourceExhausted
    );
    registry.reserve("bob").unwrap();
    assert_eq!(
        registry.reserve("carol").unwrap_err().code(),
        Code::ResourceExhausted
    );
    registry.release_reservation("alice");
    let id = registry
        .register("alice".into(), tx("a", 1), mailbox(&cfg))
        .unwrap();
    assert!(registry.reserve("alice").is_err());
    registry.remove(id, Status::cancelled("test"));
    registry.reserve("alice").unwrap();
    registry.release_reservation("alice");
    registry.release_reservation("bob");
    registry.reserve("carol").unwrap();
}

#[test]
fn configuration_rejects_unusable_limits_and_public_metrics() {
    assert!(SharedConfig {
        max_request_bytes: 1,
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(SharedConfig {
        metrics_listen: Some("0.0.0.0:9090".parse().unwrap()),
        ..Default::default()
    }
    .validate()
    .is_err());
    let template: crate::config::Config =
        toml::from_str(include_str!("../../config.shared-receiver.toml")).unwrap();
    template.grpc.unwrap().shared.unwrap().validate().unwrap();
}
