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
async fn identical_requests_are_free_but_alias_changes_still_update_routes() {
    let cfg = SharedConfig {
        max_subscription_changes_per_second: 2,
        ..Default::default()
    };
    let (hub, mut changes) = Registry::new(cfg.clone(), Arc::default());
    let output = mailbox(&cfg);
    let id = hub
        .lock()
        .unwrap()
        .register("alice".into(), tx("original", 1), output.clone())
        .unwrap();
    let label = changes
        .borrow_and_update()
        .transactions
        .keys()
        .next()
        .unwrap()
        .clone();
    for _ in 0..200 {
        hub.lock().unwrap().update(id, tx("original", 1)).unwrap();
    }
    assert!(!changes.has_changed().unwrap());
    // Same upstream filter, different downstream alias: this is a real change.
    hub.lock().unwrap().update(id, tx("renamed", 1)).unwrap();
    assert!(!changes.has_changed().unwrap());
    assert_eq!(
        hub.lock()
            .unwrap()
            .update(id, tx("excess", 2))
            .unwrap_err()
            .code(),
        Code::ResourceExhausted
    );
    hub.lock().unwrap().dispatch(update(label));
    assert_eq!(output.recv().await.unwrap().filters, vec!["renamed"]);
    hub.lock().unwrap().remove(id, Status::cancelled("test"));
    assert_eq!(
        hub.lock()
            .unwrap()
            .update(id, tx("renamed", 1))
            .unwrap_err()
            .code(),
        Code::Cancelled
    );
}

#[test]
fn change_limit_spans_streams_and_reconnects_but_isolates_users() {
    let cfg = SharedConfig {
        max_subscription_changes_per_second: 3,
        ..Default::default()
    };
    let (hub, _) = Registry::new(cfg.clone(), Arc::default());
    let mut registry = hub.lock().unwrap();
    let a = registry
        .register("alice".into(), tx("a", 1), mailbox(&cfg))
        .unwrap();
    let b = registry
        .register("alice".into(), tx("b", 1), mailbox(&cfg))
        .unwrap();
    registry.update(a, tx("changed", 2)).unwrap();
    assert_eq!(
        registry.update(b, tx("changed", 2)).unwrap_err().code(),
        Code::ResourceExhausted
    );
    registry.remove(a, Status::cancelled("test"));
    registry.remove(b, Status::cancelled("test"));
    assert_eq!(
        registry
            .register("alice".into(), tx("again", 1), mailbox(&cfg))
            .unwrap_err()
            .code(),
        Code::ResourceExhausted
    );
    registry
        .register("bob".into(), tx("bob", 1), mailbox(&cfg))
        .unwrap();
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
    assert!(filters::validate(&request, &cfg, "bob").is_ok());
    request
        .transactions
        .get_mut("tx")
        .unwrap()
        .account_include
        .clear();
    assert_eq!(
        filters::validate(&request, &cfg, "bob").unwrap_err().code(),
        Code::PermissionDenied
    );
    request.transactions.get_mut("tx").unwrap().account_exclude =
        vec![bs58::encode([1; 32]).into_string()];
    assert!(filters::validate(&request, &cfg, "bob").is_err());
    request = tx("tx", 1);
    request.transactions.get_mut("tx").unwrap().account_include = vec!["bad".into()];
    assert!(filters::validate(&request, &cfg, "bob").is_err());
    request = tx("tx", 1);
    request.from_slot = Some(1);
    assert!(filters::validate(&request, &cfg, "bob").is_err());
    request = tx("tx", 1);
    request
        .accounts_data_slice
        .push(SubscribeRequestAccountsDataSlice {
            offset: 0,
            length: 1,
        });
    assert!(filters::validate(&request, &cfg, "bob").is_err());
    request = tx("tx", 1);
    request.commitment = Some(CommitmentLevel::Finalized as i32);
    assert!(filters::validate(&request, &cfg, "bob").is_err());
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
            },
            "bob",
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
    for rate in [0, 1001] {
        assert!(SharedConfig {
            max_subscription_changes_per_second: rate,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    for limits in [
        UserLimits {
            max_subscriptions: Some(0),
            ..Default::default()
        },
        UserLimits {
            max_filters: Some(4097),
            ..Default::default()
        },
        UserLimits {
            max_accounts: Some(65537),
            ..Default::default()
        },
    ] {
        assert!(SharedConfig {
            users: [("alice".into(), limits)].into(),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
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

// Run manually on an otherwise idle host; excludes socket I/O and protobuf encoding.
#[tokio::test]
#[ignore = "manual forwarding microbenchmark"]
async fn benchmark_shared_hot_paths() {
    fn report(name: &str, mut samples: Vec<u128>) {
        samples.sort_unstable();
        eprintln!(
            "{name}: p50={}ns p95={}ns p99={}ns",
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            samples[samples.len() * 99 / 100]
        );
    }
    let cfg = SharedConfig {
        max_subscription_changes_per_second: 1000,
        ..Default::default()
    };
    let (hub, changes) = Registry::new(cfg.clone(), Arc::default());
    let mut outputs = Vec::new();
    let mut first = 0;
    let mut request = SubscribeRequest::default();
    for i in 0..8 {
        request
            .transactions
            .extend(tx(&format!("filter{i}"), i).transactions);
    }
    for i in 0..64 {
        let output = mailbox(&cfg);
        let id = hub
            .lock()
            .unwrap()
            .register(format!("user{i}"), request.clone(), output.clone())
            .unwrap();
        if i == 0 {
            first = id;
        }
        outputs.push(output);
    }
    let label = changes.borrow().transactions.keys().next().unwrap().clone();
    let mut samples = Vec::new();
    for i in 0..532 {
        let value = update(label.clone());
        let start = std::time::Instant::now();
        hub.lock().unwrap().dispatch(value);
        let elapsed = start.elapsed().as_nanos();
        for output in &outputs {
            output.recv().await.unwrap();
        }
        if i >= 32 {
            samples.push(elapsed);
        }
    }
    report("fanout_64", samples);
    let mut samples = Vec::new();
    for i in 0..232 {
        let mut next = request.clone();
        let filter = next.transactions.remove("filter0").unwrap();
        next.transactions.insert(format!("alias{i}"), filter);
        let start = std::time::Instant::now();
        hub.lock().unwrap().update(first, next).unwrap();
        if i >= 32 {
            samples.push(start.elapsed().as_nanos());
        }
    }
    report("subscription_change_64x8", samples);
}

#[tokio::test]
async fn dispatch_is_independent_of_management_lock_and_merges_matching_aliases() {
    let cfg = SharedConfig::default();
    let metrics = Arc::new(Metrics::default());
    let (hub, changes) = Registry::new(cfg.clone(), metrics.clone());
    let output = mailbox(&cfg);
    let mut request = tx("first", 1);
    request
        .transactions
        .extend(tx("same-content", 1).transactions);
    request.transactions.extend(tx("second", 2).transactions);
    {
        let mut registry = hub.lock().unwrap();
        registry
            .register("alice".into(), request, output.clone())
            .unwrap();
        let mut labels: Vec<_> = changes.borrow().transactions.keys().cloned().collect();
        labels.push(labels[0].clone());
        let mut value = update(labels[0].clone());
        value.filters = labels;
        // Keep the registry locked: the production dispatch path must never acquire it.
        assert!(registry.dispatcher.dispatch(value).is_empty());
    }
    let mut names = output.recv().await.unwrap().filters;
    names.sort();
    assert_eq!(names, vec!["first", "same-content", "second"]);
    assert_eq!(metrics.delivered.load(Ordering::Relaxed), 1);
    assert!(timeout(Duration::from_millis(20), output.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn overflow_is_terminal_before_batched_cleanup_even_when_filters_change() {
    let cfg = SharedConfig {
        client_queue_capacity: 1,
        ..Default::default()
    };
    let metrics = Arc::new(Metrics::default());
    let (hub, changes) = Registry::new(cfg.clone(), metrics.clone());
    let outputs: Vec<_> = (0..4).map(|_| mailbox(&cfg)).collect();
    let ids: Vec<_> = outputs
        .iter()
        .enumerate()
        .map(|(i, output)| {
            hub.lock()
                .unwrap()
                .register(format!("user{i}"), tx("old", 1), output.clone())
                .unwrap()
        })
        .collect();
    let label = changes.borrow().transactions.keys().next().unwrap().clone();
    let dispatcher = hub.lock().unwrap().dispatcher.clone();
    assert!(dispatcher.dispatch(update(label.clone())).is_empty());
    outputs[3].recv().await.unwrap();
    let slow = dispatcher.dispatch(update(label.clone()));
    assert_eq!(slow.len(), 3);
    // Do not silently resume after dropping an update: overflow closes the stream
    // immediately, before cleanup can acquire the management lock.
    assert_eq!(
        hub.lock()
            .unwrap()
            .update(ids[0], tx("new", 1))
            .unwrap_err()
            .code(),
        Code::Cancelled
    );
    for output in &outputs[..3] {
        assert!(*output.closed.borrow());
    }
    // The response task may drop its session before the dispatcher obtains the lock.
    hub.lock()
        .unwrap()
        .remove(ids[1], Status::cancelled("response ended"));
    hub.lock().unwrap().remove_slow(slow);
    assert_eq!(metrics.slow_clients.load(Ordering::Relaxed), 3);
    assert_eq!(metrics.subscriptions.load(Ordering::Relaxed), 1);
    for output in &outputs[..3] {
        assert_eq!(
            output.recv().await.unwrap_err().code(),
            Code::ResourceExhausted
        );
    }
    assert_eq!(outputs[3].recv().await.unwrap().filters, vec!["old"]);
    assert!(dispatcher.dispatch(update(label)).is_empty());
    assert_eq!(outputs[3].recv().await.unwrap().filters, vec!["old"]);
}

#[tokio::test]
async fn mailbox_reuses_last_payload_but_isolates_each_consumers_data_and_aliases() {
    fn account(update: &mut SubscribeUpdate) -> &mut Vec<u8> {
        let Some(subscribe_update::UpdateOneof::Account(account)) = &mut update.update_oneof else {
            panic!("expected account update");
        };
        &mut account.account.as_mut().unwrap().data
    }
    let cfg = SharedConfig::default();
    let budget = Arc::new(Semaphore::new(cfg.total_queue_bytes));
    let a = Mailbox::new(&cfg, budget.clone());
    let b = Mailbox::new(&cfg, budget.clone());
    let payload = vec![7; 128 * 1024];
    let original_address = payload.as_ptr();
    let update = Arc::new(SubscribeUpdate {
        filters: vec!["upstream-name".into()],
        update_oneof: Some(subscribe_update::UpdateOneof::Account(
            SubscribeUpdateAccount {
                account: Some(SubscribeUpdateAccountInfo {
                    data: payload,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
        ..Default::default()
    });
    let bytes = update.encoded_len();
    assert!(matches!(
        a.send(update.clone(), Arc::from(["alice".into()]), bytes),
        Delivery::Sent
    ));
    assert!(matches!(
        b.send(update, Arc::from(["bob".into()]), bytes),
        Delivery::Sent
    ));
    let mut first = a.recv().await.unwrap();
    assert_eq!(first.filters, ["alice"]);
    assert_ne!(account(&mut first).as_ptr(), original_address);
    account(&mut first)[0] = 99;
    let mut last = b.recv().await.unwrap();
    assert_eq!(last.filters, ["bob"]);
    assert_eq!(account(&mut last).as_ptr(), original_address);
    assert_eq!(account(&mut last)[0], 7);
    assert_eq!(budget.available_permits(), cfg.total_queue_bytes);
}

// Includes mailbox enqueue/dequeue and payload ownership; excludes wire encoding/network.
#[tokio::test]
#[ignore = "manual mailbox payload benchmark"]
async fn benchmark_mailbox_payload_ownership() {
    let cfg = SharedConfig::default();
    let output = mailbox(&cfg);
    let mut times = Vec::new();
    for i in 0..532 {
        let update = Arc::new(SubscribeUpdate {
            update_oneof: Some(subscribe_update::UpdateOneof::Account(
                SubscribeUpdateAccount {
                    account: Some(SubscribeUpdateAccountInfo {
                        data: vec![7; 128 * 1024],
                        ..Default::default()
                    }),
                    slot: 42,
                    ..Default::default()
                },
            )),
            ..Default::default()
        });
        let bytes = update.encoded_len();
        let names = Arc::from(["account".to_owned()]);
        let start = std::time::Instant::now();
        assert!(matches!(output.send(update, names, bytes), Delivery::Sent));
        let received = output.recv().await.unwrap();
        let elapsed = start.elapsed().as_nanos();
        assert!(received.update_oneof.is_some());
        if i >= 32 {
            times.push(elapsed);
        }
    }
    times.sort_unstable();
    eprintln!(
        "mailbox_128k_single: p50={}ns p95={}ns p99={}ns",
        times[250], times[475], times[495]
    );
}
