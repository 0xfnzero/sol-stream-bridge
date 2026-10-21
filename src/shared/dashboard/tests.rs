use super::*;
use crate::shared::{
    state::{Mailbox, Registry},
    Service,
};
use axum::{body::to_bytes, http::Request as HttpRequest};
use std::sync::atomic::Ordering;
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tower::ServiceExt;
use yellowstone_grpc_proto::{
    prelude::*,
    tonic::{self, metadata::MetadataMap, Code},
};

const TOKEN: &str = "test-administrator-token-32-bytes-minimum";
struct Fixture {
    admin: Admin,
    _dir: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let metrics = Arc::new(Metrics::default());
        let (hub, _) = Registry::new(SharedConfig::default(), metrics.clone());
        let state = ManagedState {
            require_api_key: true,
            allowed_sources: vec!["127.0.0.1".parse().unwrap()],
            users: [(
                "alice".into(),
                ManagedUser {
                    key: "alice-test-key".into(),
                    enabled: true,
                    limits: Default::default(),
                },
            )]
            .into(),
            ..Default::default()
        };
        hub.lock()
            .unwrap()
            .apply_access(AccessPolicy::new(state).unwrap());
        Self {
            admin: Admin {
                hub,
                metrics,
                token: Sha256::digest(TOKEN).into(),
                writer: Arc::new(Mutex::new(
                    persistence::Writer::open(dir.path().join("state.toml")).unwrap(),
                )),
                assets: Arc::new(
                    [(
                        "/index.html".into(),
                        ("text/html", Bytes::from_static(b"<html>test</html>")),
                    )]
                    .into(),
                ),
                info: Arc::new(json!({})),
                started: Instant::now(),
            },
            _dir: dir,
        }
    }
    async fn call(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let request = HttpRequest::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost:8080")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router(self.admin.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }
    fn authenticate(&self, key: &str) -> Result<String, tonic::Status> {
        let mut metadata = MetadataMap::new();
        metadata.insert("x-token", key.parse().unwrap());
        self.admin
            .hub
            .lock()
            .unwrap()
            .access
            .authenticate(&metadata, "127.0.0.1".parse().unwrap())
    }
}

#[tokio::test]
async fn admin_auth_origin_and_redaction() {
    let f = Fixture::new();
    for (host, origin, tokens, expected) in [
        ("localhost:8080", None, vec![], StatusCode::UNAUTHORIZED),
        (
            "localhost:8080",
            None,
            vec!["wrong-token-longer-than-thirty-two-bytes"],
            StatusCode::UNAUTHORIZED,
        ),
        (
            "localhost:8080",
            None,
            vec![TOKEN, TOKEN],
            StatusCode::UNAUTHORIZED,
        ),
        ("evil.example", None, vec![TOKEN], StatusCode::FORBIDDEN),
        (
            "localhost:8080",
            Some("http://evil.example"),
            vec![TOKEN],
            StatusCode::FORBIDDEN,
        ),
        (
            "127.0.0.1:18080",
            Some("http://127.0.0.1:18080"),
            vec![TOKEN],
            StatusCode::OK,
        ),
    ] {
        let mut req = HttpRequest::builder()
            .uri("/api/state")
            .header("host", host);
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        for token in tokens {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let res = router(f.admin.clone())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), expected);
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert!(res.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'"));
        let body = to_bytes(res.into_body(), 1_000_000).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(!body.contains(TOKEN) && !body.contains("alice-test-key"));
    }
    let res = router(f.admin.clone())
        .oneshot(
            HttpRequest::builder()
                .uri("/")
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let req = HttpRequest::builder()
        .method("POST")
        .uri("/api/users")
        .header("host", "localhost")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from("x".repeat(33 * 1024)))
        .unwrap();
    assert_eq!(
        router(f.admin.clone()).oneshot(req).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn lifecycle_is_durable_revision_checked_and_never_fails_open() {
    let f = Fixture::new();
    let (code, created) = f
        .call(
            "POST",
            "/api/users",
            json!({"revision":"0","name":"bob","limits":{"max_subscriptions":1}}),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let old_key = created["api_key"].as_str().unwrap();
    assert_eq!(old_key.len(), 64);
    assert_eq!(f.authenticate(old_key).unwrap(), "bob");
    let (code, _) = f
        .call(
            "PUT",
            "/api/users/bob",
            json!({"revision":"0","enabled":false,"limits":{}}),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);
    let (code, _) = f
        .call(
            "PUT",
            "/api/users/bob",
            json!({"revision":"1","enabled":true,"limits":{"max_accounts":0}}),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, rotated) = f
        .call("POST", "/api/users/bob/rotate", json!({"revision":"1"}))
        .await;
    assert_eq!(code, StatusCode::OK);
    let key = rotated["api_key"].as_str().unwrap();
    assert_ne!(key, old_key);
    assert!(f.authenticate(old_key).is_err());
    assert_eq!(f.authenticate(key).unwrap(), "bob");
    let (code, _) = f
        .call(
            "PUT",
            "/api/users/bob",
            json!({"revision":"2","enabled":false,"limits":{}}),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert!(f.authenticate(key).is_err());
    assert_eq!(
        f.call("DELETE", "/api/users/bob", json!({"revision":"3"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.call("DELETE", "/api/users/alice", json!({"revision":"4"}))
            .await
            .0,
        StatusCode::OK
    );
    let path = f.admin.writer.lock().await.path.clone();
    let loaded = persistence::read(&path).unwrap().unwrap();
    assert!(loaded.require_api_key && loaded.users.is_empty());
    assert_eq!(loaded.revision, 5);
    let restarted = AccessPolicy::new(loaded).unwrap();
    assert_eq!(
        restarted
            .authenticate(&MetadataMap::new(), "127.0.0.1".parse().unwrap())
            .unwrap_err()
            .code(),
        Code::Unauthenticated
    );
    assert_eq!(
        f.call(
            "PUT",
            "/api/access",
            json!({"revision":"5","require_api_key":false,"allowed_sources":["127.0.0.1"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.authenticate("").unwrap(), "ip:127.0.0.1");
}

fn subscribe_request() -> SubscribeRequest {
    SubscribeRequest {
        transactions: [(
            "bot".into(),
            SubscribeRequestFilterTransactions {
                account_include: vec![bs58::encode([1; 32]).into_string()],
                ..Default::default()
            },
        )]
        .into(),
        ..Default::default()
    }
}
#[tokio::test]
async fn policy_updates_only_disconnect_affected_streams() {
    let f = Fixture::new();
    let cfg = SharedConfig::default();
    let a = Mailbox::new(&cfg, Arc::new(Semaphore::new(cfg.total_queue_bytes)));
    let b = Mailbox::new(&cfg, Arc::new(Semaphore::new(cfg.total_queue_bytes)));
    let bob;
    {
        let mut hub = f.admin.hub.lock().unwrap();
        let mut managed = hub.access.state.clone();
        managed.allowed_sources.push("127.0.0.2".parse().unwrap());
        managed.users.insert(
            "bob".into(),
            ManagedUser {
                key: "bob-test-key".into(),
                enabled: true,
                limits: Default::default(),
            },
        );
        hub.apply_access(AccessPolicy::new(managed).unwrap());
        hub.register_peer(
            "alice".into(),
            "127.0.0.1".parse().unwrap(),
            subscribe_request(),
            a.clone(),
        )
        .unwrap();
        bob = hub
            .register_peer(
                "bob".into(),
                "127.0.0.2".parse().unwrap(),
                subscribe_request(),
                b.clone(),
            )
            .unwrap();
    }
    assert_eq!(
        f.call("POST", "/api/users/alice/rotate", json!({"revision":"0"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(a.recv().await.unwrap_err().code(), Code::PermissionDenied);
    assert_eq!(f.admin.metrics.subscriptions.load(Ordering::Relaxed), 1);
    assert!(tokio::time::timeout(Duration::from_millis(10), b.recv())
        .await
        .is_err());
    assert_eq!(
        f.call(
            "PUT",
            "/api/access",
            json!({"revision":"1","require_api_key":true,"allowed_sources":["127.0.0.1"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(b.recv().await.unwrap_err().code(), Code::PermissionDenied);
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/subscriptions/{bob}/disconnect"),
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let c = Mailbox::new(&cfg, Arc::new(Semaphore::new(cfg.total_queue_bytes)));
    let id = f
        .admin
        .hub
        .lock()
        .unwrap()
        .register("alice".into(), subscribe_request(), c.clone())
        .unwrap();
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/subscriptions/{id}/disconnect"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(c.recv().await.unwrap_err().code(), Code::Cancelled);
}

#[tokio::test]
async fn pending_grpc_admission_cannot_use_revoked_key() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = SharedConfig::default();
    let service = Service {
        hub: f.admin.hub.clone(),
        cfg: cfg.clone(),
        metrics: f.admin.metrics.clone(),
        queue_bytes: Arc::new(Semaphore::new(cfg.total_queue_bytes)),
        initial_timeout: Duration::from_secs(3),
    };
    let server = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(geyser_server::GeyserServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let mut client = geyser_client::GeyserClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let (tx, rx) = mpsc::channel(1);
    let mut req = tonic::Request::new(ReceiverStream::new(rx));
    req.metadata_mut()
        .insert("x-token", "alice-test-key".parse().unwrap());
    let pending = tokio::spawn(async move { client.subscribe(req).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.admin.hub.lock().unwrap().users()[0]["usage"]["pending"] == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.call("POST", "/api/users/alice/rotate", json!({"revision":"0"}))
            .await
            .0,
        StatusCode::OK
    );
    tx.send(subscribe_request()).await.unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap_err().code(),
        Code::Unauthenticated
    );
    assert_eq!(f.admin.metrics.subscriptions.load(Ordering::Relaxed), 0);
    server.abort();
}

#[tokio::test]
async fn external_edits_block_writes_without_changing_runtime() {
    let f = Fixture::new();
    let path = f.admin.writer.lock().await.path.clone();
    assert!(persistence::Writer::open(path.clone()).is_err());
    persistence::save(&path, None, "invalid state").unwrap();
    assert!(persistence::read(&path).is_err());
    assert_eq!(
        f.call("DELETE", "/api/users/alice", json!({"revision":"0"}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.authenticate("alice-test-key").unwrap(), "alice");
    assert_eq!(f.admin.hub.lock().unwrap().access.state.revision, 0);
}
#[cfg(unix)]
#[test]
fn unsafe_state_files_fail_closed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    let contents = toml::to_string(&ManagedState::default()).unwrap();
    persistence::save(&path, None, &contents).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(persistence::read(&path).is_err());
    let link = dir.path().join("linked.toml");
    symlink(&path, &link).unwrap();
    assert!(persistence::read(&link).is_err());
    std::fs::remove_file(link).unwrap();
    symlink(&path, dir.path().join("linked.toml.lock")).unwrap();
    assert!(persistence::Writer::open(dir.path().join("linked.toml")).is_err());
    assert!(DashboardConfig {
        listen: "0.0.0.0:8080".parse().unwrap(),
        ..Default::default()
    }
    .validate()
    .is_err());
}

#[tokio::test]
async fn management_write_admission_is_bounded_and_names_are_addressable() {
    let f = Fixture::new();
    let guard = f.admin.writer.lock().await;
    assert_eq!(
        f.call("DELETE", "/api/users/alice", json!({"revision":"0"}))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(guard);
    for name in [".", "..", "", "a/b"] {
        assert_eq!(
            f.call(
                "POST",
                "/api/users",
                json!({"revision":"0", "name":name, "limits":{}})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(f.authenticate("alice-test-key").unwrap(), "alice");
}
