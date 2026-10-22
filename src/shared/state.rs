use super::{
    access::{AccessPolicy, ManagedState},
    config::ResolvedLimits,
    dispatch::{Destination, Dispatcher},
    filters,
    metrics::Metrics,
    SharedConfig,
};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};
use yellowstone_grpc_proto::{prelude::*, tonic::Status};

struct Queued {
    update: Arc<SubscribeUpdate>,
    filters: Arc<[String]>,
    _local: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Queued>,
    error: Option<Status>,
}
pub enum Delivery {
    Sent,
    Ignored,
    Full,
}
pub struct Mailbox {
    queue: Mutex<Queue>,
    notify: Notify,
    capacity: usize,
    bytes: Arc<Semaphore>,
    global: Arc<Semaphore>,
    pub closed: watch::Sender<bool>,
    pub delivered: AtomicU64,
}
impl Mailbox {
    pub fn new(cfg: &SharedConfig, global: Arc<Semaphore>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Queue::default()),
            notify: Notify::new(),
            capacity: cfg.client_queue_capacity,
            bytes: Arc::new(Semaphore::new(cfg.client_queue_bytes)),
            global,
            closed: watch::channel(false).0,
            delivered: AtomicU64::new(0),
        })
    }
    pub fn send(
        &self,
        update: Arc<SubscribeUpdate>,
        filters: Arc<[String]>,
        encoded_bytes: usize,
    ) -> Delivery {
        let mut queue = self.queue.lock().unwrap();
        if queue.error.is_some() {
            return Delivery::Ignored;
        }
        if queue.items.len() >= self.capacity {
            return Delivery::Full;
        }
        let bytes = encoded_bytes
            .saturating_add(filters.iter().map(|s| s.len() + 8).sum::<usize>())
            .max(1);
        let Ok(bytes) = u32::try_from(bytes) else {
            return Delivery::Full;
        };
        let Ok(local) = self.bytes.clone().try_acquire_many_owned(bytes) else {
            return Delivery::Full;
        };
        let Ok(global) = self.global.clone().try_acquire_many_owned(bytes) else {
            return Delivery::Full;
        };
        queue.items.push_back(Queued {
            update,
            filters,
            _local: local,
            _global: global,
        });
        drop(queue);
        self.notify.notify_one();
        Delivery::Sent
    }
    /// Returns true only for the first termination, so concurrent cleanup counts once.
    pub fn fail(&self, error: Status) -> bool {
        let mut queue = self.queue.lock().unwrap();
        if queue.error.is_some() {
            return false;
        }
        queue.error = Some(error);
        // Release queued data and budgets immediately, even if the peer stops reading.
        let abandoned = std::mem::take(&mut queue.items);
        drop(queue);
        self.closed.send_replace(true);
        self.notify.notify_one();
        drop(abandoned);
        true
    }
    pub async fn recv(&self) -> Result<SubscribeUpdate, Status> {
        loop {
            let notified = self.notify.notified();
            let item = {
                let mut queue = self.queue.lock().unwrap();
                if let Some(error) = &queue.error {
                    return Err(error.clone());
                }
                queue.items.pop_front()
            };
            // Move the body when this is its last owner (especially one Bot); otherwise
            // clone outside the lock so consumers can mutate their own protobuf safely.
            if let Some(item) = item {
                let mut update = Arc::unwrap_or_clone(item.update);
                update.filters = item.filters.to_vec();
                return Ok(update);
            }
            notified.await;
        }
    }
}
struct Client {
    user: String,
    peer: IpAddr,
    started_at_ms: u64,
    request: SubscribeRequest,
    prepared: filters::Aggregate,
    mailbox: Arc<Mailbox>,
}
pub struct Registry {
    pub cfg: SharedConfig,
    pub access: AccessPolicy,
    clients: BTreeMap<u64, Client>,
    pending: HashMap<String, usize>,
    change_windows: HashMap<String, (Instant, usize)>,
    pub dispatcher: Arc<Dispatcher>,
    upstream: watch::Sender<Arc<SubscribeRequest>>,
    pub metrics: Arc<Metrics>,
    next_id: u64,
}
pub type Hub = Arc<Mutex<Registry>>;
impl Registry {
    pub fn new(
        cfg: SharedConfig,
        metrics: Arc<Metrics>,
    ) -> (Hub, watch::Receiver<Arc<SubscribeRequest>>) {
        let request = filters::aggregate(std::iter::empty(), &cfg)
            .expect("validated config")
            .request;
        let (upstream, rx) = watch::channel(Arc::new(request));
        (
            Arc::new(Mutex::new(Self {
                cfg,
                access: AccessPolicy::new(ManagedState::default()).expect("empty access policy"),
                clients: BTreeMap::new(),
                pending: HashMap::new(),
                change_windows: HashMap::new(),
                dispatcher: Arc::new(Dispatcher::new(metrics.clone())),
                upstream,
                metrics,
                next_id: 0,
            })),
            rx,
        )
    }
    pub fn reserve(&mut self, user: &str) -> Result<(), Status> {
        let limit = self
            .cfg
            .users
            .get(user)
            .and_then(|limits| limits.max_subscriptions)
            .unwrap_or(self.cfg.max_subscriptions_per_key);
        if self.clients.len() + self.pending.values().sum::<usize>() >= self.cfg.max_subscriptions
            || self
                .clients
                .values()
                .filter(|client| client.user == user)
                .count()
                + self.pending.get(user).copied().unwrap_or(0)
                >= limit
        {
            return Err(Status::resource_exhausted("subscription limit reached"));
        }
        *self.pending.entry(user.to_owned()).or_default() += 1;
        Ok(())
    }
    pub fn release_reservation(&mut self, user: &str) {
        if let Some(count) = self.pending.get_mut(user) {
            *count -= 1;
            if *count == 0 {
                self.pending.remove(user);
            }
        }
    }
    fn charge_change(&mut self, user: &str, now: Instant) -> Result<(), Status> {
        // Retain recent disconnected users so reconnecting cannot reset the limit.
        // Expired entries are discarded when admitting a new identity.
        if !self.change_windows.contains_key(user) {
            self.change_windows
                .retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(1));
        }
        let (start, count) = self
            .change_windows
            .entry(user.to_owned())
            .or_insert((now, 0));
        if now.duration_since(*start) >= Duration::from_secs(1) {
            *start = now;
            *count = 0;
        }
        if *count >= self.cfg.max_subscription_changes_per_second {
            return Err(Status::resource_exhausted(
                "subscription change rate limit reached; retry after one second",
            ));
        }
        *count += 1;
        Ok(())
    }
    fn publish(&mut self, aggregate: filters::Aggregate) {
        self.metrics
            .subscriptions
            .store(self.clients.len(), Ordering::Relaxed);
        self.metrics
            .filters
            .store(filters::filter_count(&aggregate.request), Ordering::Relaxed);
        self.metrics
            .accounts
            .store(aggregate.accounts, Ordering::Relaxed);
        let routes = aggregate
            .routes
            .into_iter()
            .map(|(label, aliases)| {
                let mut grouped = BTreeMap::<u64, Vec<String>>::new();
                for (id, name) in aliases {
                    grouped.entry(id).or_default().push(name);
                }
                let destinations = grouped
                    .into_iter()
                    .map(|(id, aliases)| {
                        let client = &self.clients[&id];
                        Destination {
                            id,
                            mailbox: client.mailbox.clone(),
                            aliases: aliases.into(),
                        }
                    })
                    .collect();
                (label, destinations)
            })
            .collect();
        self.dispatcher.publish(routes);
        // No redundant update when only downstream aliases/consumers change.
        if **self.upstream.borrow() != aggregate.request {
            self.upstream.send_replace(Arc::new(aggregate.request));
        }
    }
    fn check_user_budget(
        &self,
        user: &str,
        replacing: Option<u64>,
        request: &SubscribeRequest,
        limits: &ResolvedLimits,
    ) -> Result<(), Status> {
        let (mut filters, mut accounts) = (
            filters::filter_count(request),
            filters::account_count(request),
        );
        for (id, client) in &self.clients {
            if client.user == user && Some(*id) != replacing {
                filters += filters::filter_count(&client.request);
                accounts += filters::account_count(&client.request);
            }
        }
        if filters > limits.max_filters_per_key || accounts > limits.max_accounts_per_key {
            return Err(Status::resource_exhausted(
                "user filter/account limit reached",
            ));
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn register(
        &mut self,
        user: String,
        request: SubscribeRequest,
        mailbox: Arc<Mailbox>,
    ) -> Result<u64, Status> {
        self.register_peer(user, "127.0.0.1".parse().unwrap(), request, mailbox)
    }
    pub fn register_peer(
        &mut self,
        user: String,
        peer: IpAddr,
        request: SubscribeRequest,
        mailbox: Arc<Mailbox>,
    ) -> Result<u64, Status> {
        self.charge_change(&user, Instant::now())?;
        let limits = self.cfg.for_user(&user);
        filters::validate(&request, &self.cfg, &user)?;
        self.check_user_budget(&user, None, &request, &limits)?;
        if self.clients.len() >= self.cfg.max_subscriptions
            || self.clients.values().filter(|c| c.user == user).count() >= limits.max_subscriptions
        {
            return Err(Status::resource_exhausted("subscription limit reached"));
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Status::resource_exhausted("subscription IDs exhausted"))?;
        // Check the candidate union before mutating any existing subscription.
        let prepared = filters::aggregate(std::iter::once((id, &request)), &self.cfg)?;
        let aggregate = filters::combine(
            self.clients
                .values()
                .map(|c| &c.prepared)
                .chain(std::iter::once(&prepared)),
            &self.cfg,
        )?;
        self.next_id = id;
        self.clients.insert(
            id,
            Client {
                user,
                peer: stream_bridge_client::canonical_ip(peer),
                started_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                request,
                prepared,
                mailbox,
            },
        );
        self.publish(aggregate);
        Ok(id)
    }
    pub fn update(&mut self, id: u64, request: SubscribeRequest) -> Result<(), Status> {
        let client = self
            .clients
            .get(&id)
            .ok_or_else(|| Status::cancelled("subscription closed"))?;
        // Queue overflow can close a mailbox before deferred registry cleanup.
        if *client.mailbox.closed.borrow() {
            return Err(Status::cancelled("subscription closed"));
        }
        // Access/limit changes revoke affected streams before this can run.
        if client.request == request {
            return Ok(());
        }
        let user = client.user.clone();
        self.charge_change(&user, Instant::now())?;
        let limits = self.cfg.for_user(&user);
        filters::validate(&request, &self.cfg, &user)?;
        self.check_user_budget(&user, Some(id), &request, &limits)?;
        let prepared = filters::aggregate(std::iter::once((id, &request)), &self.cfg)?;
        let aggregate = filters::combine(
            self.clients
                .iter()
                .map(|(key, c)| if *key == id { &prepared } else { &c.prepared }),
            &self.cfg,
        )?;
        let client = self.clients.get_mut(&id).unwrap();
        client.request = request;
        client.prepared = prepared;
        self.publish(aggregate);
        Ok(())
    }
    pub fn remove(&mut self, id: u64, status: Status) {
        if let Some(client) = self.clients.remove(&id) {
            client.mailbox.fail(status);
            let aggregate = filters::combine(self.clients.values().map(|c| &c.prepared), &self.cfg)
                .expect("removal cannot increase limits");
            self.publish(aggregate);
        }
    }
    pub fn fail_all(&mut self, status: Status) {
        for client in self.clients.values() {
            client.mailbox.fail(status.clone());
        }
        self.clients.clear();
        let aggregate =
            filters::aggregate(std::iter::empty(), &self.cfg).expect("validated config");
        self.publish(aggregate);
    }
    pub fn apply_access(&mut self, access: AccessPolicy) {
        let mode_changed = self.access.state.require_api_key != access.state.require_api_key;
        let revoke: Vec<_> = self
            .clients
            .iter()
            .filter(|(_, client)| {
                mode_changed
                    || !access.allows(client.peer)
                    || self.access.state.users.get(&client.user)
                        != access.state.users.get(&client.user)
            })
            .map(|(id, _)| *id)
            .collect();
        self.cfg.users = access
            .state
            .users
            .iter()
            .map(|(name, user)| (name.clone(), user.limits.clone()))
            .collect();
        self.access = access;
        if !revoke.is_empty() {
            for id in revoke {
                if let Some(client) = self.clients.remove(&id) {
                    client.mailbox.fail(Status::permission_denied(
                        "access policy changed; reconnect and resubscribe",
                    ));
                }
            }
            let aggregate = filters::combine(self.clients.values().map(|c| &c.prepared), &self.cfg)
                .expect("removal cannot increase limits");
            self.publish(aggregate);
        }
    }
    pub fn subscriptions(&self) -> Vec<serde_json::Value> {
        self.clients.iter().take(200).map(|(id, client)| serde_json::json!({
            "id": id.to_string(), "user": client.user, "peer": client.peer,
            "started_at_ms": client.started_at_ms, "delivered": client.mailbox.delivered.load(Ordering::Relaxed),
            "filters": filters::filter_count(&client.request), "accounts": filters::account_count(&client.request),
        })).collect()
    }
    pub fn users(&self) -> Vec<serde_json::Value> {
        // One pass across subscriptions; never clone the full user configuration per user.
        let mut usage = HashMap::<&str, (usize, usize, usize)>::new();
        for client in self.clients.values() {
            let item = usage.entry(&client.user).or_default();
            item.0 += 1;
            item.1 += filters::filter_count(&client.request);
            item.2 += filters::account_count(&client.request);
        }
        self.access.state.users.iter().map(|(name, user)| {
            let (subscriptions, filters, accounts) = usage.get(name.as_str()).copied().unwrap_or_default();
            let limits = &user.limits;
            serde_json::json!({"name":name, "enabled":user.enabled, "has_key":!user.key.is_empty(),
                "limits":limits, "effective": {"max_subscriptions":limits.max_subscriptions.unwrap_or(self.cfg.max_subscriptions_per_key),
                "max_filters":limits.max_filters.unwrap_or(self.cfg.max_filters_per_key),
                "max_accounts":limits.max_accounts.unwrap_or(self.cfg.max_accounts_per_key),
                "allow_accounts":limits.allow_accounts.unwrap_or(self.cfg.allow_accounts)},
                "usage":{"subscriptions":subscriptions,"pending":self.pending.get(name).copied().unwrap_or(0),"filters":filters,"accounts":accounts}})
        }).collect()
    }
    pub fn disconnect(&mut self, id: u64) -> bool {
        if !self.clients.contains_key(&id) {
            return false;
        }
        self.remove(id, Status::cancelled("disconnected by administrator"));
        true
    }
    #[cfg(test)]
    pub fn dispatch(&mut self, update: SubscribeUpdate) {
        let slow = self.dispatcher.dispatch(update);
        self.remove_slow(slow);
    }
    pub fn remove_slow(&mut self, slow: Vec<u64>) {
        let mut removed = 0;
        for id in slow {
            if let Some(client) = self.clients.remove(&id) {
                client.mailbox.fail(Status::resource_exhausted(
                    "client queue limit reached; reconnect and resubscribe",
                ));
                removed += 1;
            }
        }
        if removed != 0 {
            let aggregate = filters::combine(self.clients.values().map(|c| &c.prepared), &self.cfg)
                .expect("removal cannot increase limits");
            self.publish(aggregate);
        }
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn change_windows_expire_without_sleep_and_prune_inactive_users() {
        let cfg = SharedConfig {
            max_subscription_changes_per_second: 2,
            ..Default::default()
        };
        let (hub, _) = Registry::new(cfg, Arc::default());
        let mut registry = hub.lock().unwrap();
        let now = Instant::now();
        registry.charge_change("alice", now).unwrap();
        registry.charge_change("alice", now).unwrap();
        assert!(registry
            .charge_change("alice", now + Duration::from_millis(999))
            .is_err());
        registry.charge_change("bob", now).unwrap();
        registry
            .charge_change("alice", now + Duration::from_secs(1))
            .unwrap();
        registry
            .charge_change("carol", now + Duration::from_secs(2))
            .unwrap();
        assert_eq!(registry.change_windows.len(), 1);
    }
}
