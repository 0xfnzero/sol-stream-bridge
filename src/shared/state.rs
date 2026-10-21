use super::{
    filters::{self, Routes},
    metrics::Metrics,
    SharedConfig,
};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{atomic::Ordering, Arc, Mutex},
};
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};
use yellowstone_grpc_proto::{prelude::*, prost::Message, tonic::Status};

struct Queued {
    update: Arc<SubscribeUpdate>,
    filters: Vec<String>,
    _local: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}
#[derive(Default)]
struct Queue {
    items: VecDeque<Queued>,
    error: Option<Status>,
}
pub struct Mailbox {
    queue: Mutex<Queue>,
    notify: Notify,
    capacity: usize,
    bytes: Arc<Semaphore>,
    global: Arc<Semaphore>,
    pub closed: watch::Sender<bool>,
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
        })
    }
    pub fn send(
        &self,
        update: Arc<SubscribeUpdate>,
        filters: Vec<String>,
        encoded_bytes: usize,
    ) -> bool {
        let mut queue = self.queue.lock().unwrap();
        if queue.error.is_some() || queue.items.len() >= self.capacity {
            return false;
        }
        let bytes = encoded_bytes
            .saturating_add(filters.iter().map(|s| s.len() + 8).sum::<usize>())
            .max(1);
        let Ok(bytes) = u32::try_from(bytes) else {
            return false;
        };
        let Ok(local) = self.bytes.clone().try_acquire_many_owned(bytes) else {
            return false;
        };
        let Ok(global) = self.global.clone().try_acquire_many_owned(bytes) else {
            return false;
        };
        queue.items.push_back(Queued {
            update,
            filters,
            _local: local,
            _global: global,
        });
        drop(queue);
        self.notify.notify_one();
        true
    }
    pub fn fail(&self, error: Status) {
        let mut queue = self.queue.lock().unwrap();
        if queue.error.is_none() {
            queue.error = Some(error);
        }
        // Release queued data and budgets immediately, even if the peer stops reading.
        queue.items.clear();
        drop(queue);
        self.closed.send_replace(true);
        self.notify.notify_one();
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
            // Large protobuf clones must not hold the queue lock and stall fanout.
            if let Some(item) = item {
                let mut update = (*item.update).clone();
                update.filters = item.filters;
                return Ok(update);
            }
            notified.await;
        }
    }
}
struct Client {
    user: String,
    request: SubscribeRequest,
    mailbox: Arc<Mailbox>,
}
pub struct Registry {
    pub cfg: SharedConfig,
    clients: BTreeMap<u64, Client>,
    pending: HashMap<String, usize>,
    routes: Routes,
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
                clients: BTreeMap::new(),
                pending: HashMap::new(),
                routes: HashMap::new(),
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
        self.routes = aggregate.routes;
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
        limits: &SharedConfig,
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
    pub fn register(
        &mut self,
        user: String,
        request: SubscribeRequest,
        mailbox: Arc<Mailbox>,
    ) -> Result<u64, Status> {
        let limits = self.cfg.for_user(&user);
        filters::validate(&request, &limits)?;
        self.check_user_budget(&user, None, &request, &limits)?;
        if self.clients.len() >= self.cfg.max_subscriptions
            || self.clients.values().filter(|c| c.user == user).count()
                >= limits.max_subscriptions_per_key
        {
            return Err(Status::resource_exhausted("subscription limit reached"));
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Status::resource_exhausted("subscription IDs exhausted"))?;
        // Check the candidate union before mutating any existing subscription.
        let aggregate = filters::aggregate(
            self.clients
                .iter()
                .map(|(id, c)| (*id, &c.request))
                .chain(std::iter::once((id, &request))),
            &self.cfg,
        )?;
        self.next_id = id;
        self.clients.insert(
            id,
            Client {
                user,
                request,
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
        let limits = self.cfg.for_user(&client.user);
        filters::validate(&request, &limits)?;
        self.check_user_budget(&client.user, Some(id), &request, &limits)?;
        let aggregate = filters::aggregate(
            self.clients
                .iter()
                .map(|(key, c)| (*key, if *key == id { &request } else { &c.request })),
            &self.cfg,
        )?;
        self.clients.get_mut(&id).unwrap().request = request;
        self.publish(aggregate);
        Ok(())
    }
    pub fn remove(&mut self, id: u64, status: Status) {
        if let Some(client) = self.clients.remove(&id) {
            client.mailbox.fail(status);
            let aggregate = filters::aggregate(
                self.clients.iter().map(|(id, c)| (*id, &c.request)),
                &self.cfg,
            )
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
    pub fn dispatch(&mut self, update: SubscribeUpdate) {
        self.metrics.updates.fetch_add(1, Ordering::Relaxed);
        let mut recipients: HashMap<u64, Vec<String>> = HashMap::new();
        for label in &update.filters {
            if let Some(routes) = self.routes.get(label) {
                for (id, name) in routes {
                    let names = recipients.entry(*id).or_default();
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
        }
        // Unlabelled/unknown updates never get broadcast to arbitrary clients.
        let encoded_bytes = update.encoded_len();
        let update = Arc::new(update);
        let mut slow = Vec::new();
        for (id, names) in recipients {
            if let Some(client) = self.clients.get(&id) {
                if client.mailbox.send(update.clone(), names, encoded_bytes) {
                    self.metrics.delivered.fetch_add(1, Ordering::Relaxed);
                } else {
                    slow.push(id);
                }
            }
        }
        for id in slow {
            self.metrics.slow_clients.fetch_add(1, Ordering::Relaxed);
            self.remove(
                id,
                Status::resource_exhausted("client queue limit reached; reconnect and resubscribe"),
            );
        }
    }
}
