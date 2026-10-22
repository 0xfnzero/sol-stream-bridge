//! Immutable routing snapshots keep control-plane work off the forwarding path.
use super::{
    metrics::Metrics,
    state::{Delivery, Mailbox},
};
use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc, RwLock},
};
use yellowstone_grpc_proto::{prelude::SubscribeUpdate, prost::Message};

pub struct Destination {
    pub id: u64,
    pub mailbox: Arc<Mailbox>,
    pub aliases: Arc<[String]>,
}
pub type Routes = HashMap<String, Vec<Destination>>;
pub struct Dispatcher {
    routes: RwLock<Arc<Routes>>,
    metrics: Arc<Metrics>,
}
impl Dispatcher {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self {
            routes: RwLock::new(Arc::new(HashMap::new())),
            metrics,
        }
    }
    pub fn publish(&self, routes: Routes) {
        // Destroy the old snapshot outside the brief swap lock.
        let old = std::mem::replace(&mut *self.routes.write().unwrap(), Arc::new(routes));
        drop(old);
    }
    pub fn dispatch(&self, update: SubscribeUpdate) -> Vec<u64> {
        // One message uses one coherent snapshot, including during subscription changes.
        // Already-started dispatches may finish with old aliases, just like queued messages.
        // Permission revocation closes the mailbox and overrides every snapshot immediately.
        let routes = self.routes.read().unwrap().clone();
        self.dispatch_snapshot(&routes, update)
    }
    fn dispatch_snapshot(&self, routes: &Routes, update: SubscribeUpdate) -> Vec<u64> {
        self.metrics.updates.fetch_add(1, Ordering::Relaxed);
        let mut slow = Vec::new();
        let mut delivered = 0;
        let bytes = update.encoded_len();
        let update = Arc::new(update);
        let mut send = |destination: &Destination, aliases: Arc<[String]>| match destination
            .mailbox
            .send(update.clone(), aliases, bytes)
        {
            Delivery::Sent => {
                destination
                    .mailbox
                    .delivered
                    .fetch_add(1, Ordering::Relaxed);
                delivered += 1;
            }
            Delivery::Full => {
                // A dropped update is terminal, even if a filter update races cleanup.
                if destination.mailbox.fail(
                    yellowstone_grpc_proto::tonic::Status::resource_exhausted(
                        "client queue limit reached; reconnect and resubscribe",
                    ),
                ) {
                    self.metrics.slow_clients.fetch_add(1, Ordering::Relaxed);
                }
                slow.push(destination.id);
            }
            Delivery::Ignored => {}
        };
        // Most provider messages match one filter. Reuse precomputed recipients/aliases.
        if update.filters.len() == 1 {
            if let Some(destinations) = routes.get(&update.filters[0]) {
                for destination in destinations {
                    send(destination, destination.aliases.clone());
                }
            }
        } else {
            let mut recipients: HashMap<u64, (&Destination, Vec<String>)> = HashMap::new();
            for label in &update.filters {
                if let Some(destinations) = routes.get(label) {
                    for destination in destinations {
                        let (_, aliases) = recipients
                            .entry(destination.id)
                            .or_insert_with(|| (destination, Vec::new()));
                        for name in destination.aliases.iter() {
                            if !aliases.contains(name) {
                                aliases.push(name.clone());
                            }
                        }
                    }
                }
            }
            for (_, (destination, aliases)) in recipients {
                send(destination, aliases.into());
            }
        }
        self.metrics
            .delivered
            .fetch_add(delivered, Ordering::Relaxed);
        slow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::SharedConfig;
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn in_flight_snapshot_completes_but_cannot_deliver_after_revocation() {
        let cfg = SharedConfig::default();
        let mailbox = Mailbox::new(&cfg, Arc::new(Semaphore::new(cfg.total_queue_bytes)));
        let dispatcher = Dispatcher::new(Arc::default());
        let routes = [(
            "label".into(),
            vec![Destination {
                id: 1,
                mailbox: mailbox.clone(),
                aliases: vec!["old".into()].into(),
            }],
        )]
        .into();
        let update = || SubscribeUpdate {
            filters: vec!["label".into()],
            ..Default::default()
        };
        dispatcher.publish(HashMap::new());
        assert!(dispatcher.dispatch_snapshot(&routes, update()).is_empty());
        assert_eq!(mailbox.recv().await.unwrap().filters, vec!["old"]);
        assert_eq!(mailbox.delivered.load(Ordering::Relaxed), 1);
        mailbox.fail(yellowstone_grpc_proto::tonic::Status::permission_denied(
            "revoked",
        ));
        assert!(dispatcher.dispatch_snapshot(&routes, update()).is_empty());
        assert_eq!(
            mailbox.recv().await.unwrap_err().code(),
            yellowstone_grpc_proto::tonic::Code::PermissionDenied
        );
    }
}
