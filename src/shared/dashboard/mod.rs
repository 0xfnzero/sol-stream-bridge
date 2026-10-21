//! Loopback-only operator API and bundled React dashboard.
mod persistence;
#[cfg(test)]
mod tests;

use super::{
    access::{AccessPolicy, ManagedState, ManagedUser},
    config::UserLimits,
    metrics::Metrics,
    state::Hub,
    SharedConfig,
};
use crate::config::Grpc;
use anyhow::{ensure, Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::Mutex};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    pub listen: SocketAddr,
    pub token_env: String,
    pub state_file: PathBuf,
    pub assets_dir: PathBuf,
}
impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".parse().unwrap(),
            token_env: "DASHBOARD_TOKEN".into(),
            state_file: "dashboard-state.toml".into(),
            assets_dir: "dashboard/dist".into(),
        }
    }
}
impl DashboardConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.listen.ip().is_loopback() && self.listen.port() > 0,
            "dashboard.listen must use a nonzero loopback port"
        );
        ensure!(
            !self.token_env.is_empty()
                && !self.state_file.as_os_str().is_empty()
                && !self.assets_dir.as_os_str().is_empty(),
            "dashboard paths/token_env must not be empty"
        );
        Ok(())
    }
    fn token(&self) -> Result<[u8; 32]> {
        let value = std::env::var(&self.token_env)
            .context("set the dashboard administrator token environment variable")?;
        ensure!(
            value.len() >= 32 && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_graphic()),
            "dashboard token must contain 32..256 visible ASCII bytes"
        );
        Ok(Sha256::digest(value.as_bytes()).into())
    }
}
pub(super) fn load_state(g: &Grpc, cfg: &SharedConfig) -> Result<ManagedState> {
    let state = match &cfg.dashboard {
        Some(admin) => {
            persistence::read(&admin.state_file)?.unwrap_or_else(|| ManagedState::from_config(g))
        }
        None => ManagedState::from_config(g),
    };
    state.validate(cfg)?;
    Ok(state)
}
pub(super) fn validate(g: &Grpc, cfg: &SharedConfig) -> Result<()> {
    if let Some(admin) = &cfg.dashboard {
        admin.token()?;
        load_state(g, cfg)?;
        ensure!(
            admin.assets_dir.join("index.html").is_file(),
            "dashboard assets missing; run npm ci && npm run build in dashboard/"
        );
    }
    Ok(())
}

type Assets = HashMap<String, (&'static str, Bytes)>;
fn assets(dir: &std::path::Path) -> Result<Assets> {
    let mut out = HashMap::new();
    let mut total = 0;
    for folder in [dir.to_path_buf(), dir.join("assets")] {
        if !folder.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let kind = match path.extension().and_then(|s| s.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("svg") => "image/svg+xml",
                Some("woff2") => "font/woff2",
                _ => continue,
            };
            let size = entry.metadata()?.len();
            total += size;
            ensure!(total <= 32 * 1024 * 1024, "dashboard assets exceed 32 MiB");
            let name = format!(
                "/{}",
                path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/")
            );
            out.insert(name, (kind, Bytes::from(std::fs::read(path)?)));
        }
    }
    ensure!(
        out.contains_key("/index.html"),
        "dashboard index.html missing"
    );
    Ok(out)
}
#[derive(Clone)]
struct Admin {
    hub: Hub,
    metrics: Arc<Metrics>,
    token: [u8; 32],
    writer: Arc<Mutex<persistence::Writer>>,
    assets: Arc<Assets>,
    info: Arc<Value>,
    started: Instant,
}
struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
type ApiResult = std::result::Result<Json<Value>, ApiError>;
fn invalid() -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        "Invalid settings; check names, limits and IP addresses.",
    )
}

async fn guard(State(admin): State<Admin>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let authority = host.parse::<axum::http::uri::Authority>();
    let local =
        authority.is_ok_and(|a| matches!(a.host(), "localhost" | "127.0.0.1" | "[::1]" | "::1"));
    let origin_ok = request
        .headers()
        .get(header::ORIGIN)
        .is_none_or(|v| v.to_str().ok() == Some(format!("http://{host}").as_str()));
    let site_ok = request
        .headers()
        .get("sec-fetch-site")
        .is_none_or(|v| v != "cross-site");
    let mut response = if !local || !origin_ok || !site_ok {
        ApiError(
            StatusCode::FORBIDDEN,
            "Use the dashboard's loopback URL or an SSH port forward.",
        )
        .into_response()
    } else if request.uri().path().starts_with("/api/")
        && !authorized(request.headers(), &admin.token)
    {
        ApiError(StatusCode::UNAUTHORIZED, "Invalid administrator token.").into_response()
    } else {
        match tokio::time::timeout(Duration::from_secs(10), next.run(request)).await {
            Ok(response) => response,
            Err(_) => ApiError(
                StatusCode::REQUEST_TIMEOUT,
                "Request timed out; refresh before retrying a change.",
            )
            .into_response(),
        }
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}
fn authorized(headers: &HeaderMap, expected: &[u8; 32]) -> bool {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(value) = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    if values.next().is_some() || !(32..=256).contains(&value.len()) {
        return false;
    }
    let actual: [u8; 32] = Sha256::digest(value.as_bytes()).into();
    bool::from(actual.ct_eq(expected))
}
async fn static_file(State(admin): State<Admin>, request: Request) -> Response {
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = if request.uri().path() == "/" {
        "/index.html"
    } else {
        request.uri().path()
    };
    match admin.assets.get(path) {
        Some((kind, data)) => {
            ([(header::CONTENT_TYPE, *kind)], Body::from(data.clone())).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn state(State(admin): State<Admin>) -> Json<Value> {
    let registry = admin.hub.lock().unwrap();
    let state = &registry.access.state;
    let users = registry.users();
    Json(
        json!({"revision":state.revision.to_string(), "require_api_key":state.require_api_key,
        "allowed_sources":state.allowed_sources,"users":users,"subscriptions":registry.subscriptions(),
        "metrics":admin.metrics.snapshot(),"service":*admin.info,"uptime_seconds":admin.started.elapsed().as_secs()}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUser {
    revision: String,
    name: String,
    limits: UserLimits,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditUser {
    revision: String,
    enabled: bool,
    limits: UserLimits,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    revision: String,
    require_api_key: bool,
    allowed_sources: Vec<std::net::IpAddr>,
}

enum Change {
    Create(String, UserLimits),
    Edit(String, bool, UserLimits),
    Rotate(String),
    Delete(String),
    Access(bool, Vec<std::net::IpAddr>),
}
fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}
fn new_key() -> std::result::Result<String, ApiError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not generate a key.",
        )
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
async fn change(admin: Admin, revision: String, action: Change) -> ApiResult {
    let expected = revision.parse::<u64>().map_err(|_| invalid())?;
    // Admit one durable write at a time; timed-out callers cannot build an
    // unbounded queue of background mutations behind slow storage.
    let writer = admin.writer.clone().try_lock_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Another change is being saved. Refresh and try again.",
        )
    })?;
    // Finish durable mutations even if the HTTP caller disconnects midway through.
    tokio::spawn(apply_change(admin, writer, expected, action))
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Management task failed."))?
}
async fn apply_change(
    admin: Admin,
    mut writer: tokio::sync::OwnedMutexGuard<persistence::Writer>,
    expected: u64,
    action: Change,
) -> ApiResult {
    let (mut candidate, cfg) = {
        let hub = admin.hub.lock().unwrap();
        (hub.access.state.clone(), hub.cfg.clone())
    };
    if candidate.revision != expected {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Settings changed in another session. Refresh and try again.",
        ));
    }
    let mut key = None;
    match action {
        Change::Create(name, limits) => {
            if !name_valid(&name) || candidate.users.contains_key(&name) {
                return Err(invalid());
            }
            let generated = new_key()?;
            candidate.users.insert(
                name,
                ManagedUser {
                    key: generated.clone(),
                    enabled: true,
                    limits,
                },
            );
            key = Some(generated);
        }
        Change::Edit(name, enabled, limits) => {
            let user = candidate
                .users
                .get_mut(&name)
                .ok_or(ApiError(StatusCode::NOT_FOUND, "User not found."))?;
            if enabled && user.key.is_empty() && !name.starts_with("ip:") {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "Rotate this user's key before enabling it.",
                ));
            }
            user.enabled = enabled;
            user.limits = limits;
        }
        Change::Rotate(name) => {
            let user = candidate
                .users
                .get_mut(&name)
                .ok_or(ApiError(StatusCode::NOT_FOUND, "User not found."))?;
            let generated = new_key()?;
            user.key = generated.clone();
            key = Some(generated);
        }
        Change::Delete(name) => {
            if candidate.users.remove(&name).is_none() {
                return Err(ApiError(StatusCode::NOT_FOUND, "User not found."));
            }
        }
        Change::Access(required, ips) => {
            candidate.require_api_key = required;
            candidate.allowed_sources = ips;
        }
    }
    candidate.revision = candidate.revision.checked_add(1).ok_or_else(invalid)?;
    candidate.validate(&cfg).map_err(|_| invalid())?;
    let policy = AccessPolicy::new(candidate.clone()).map_err(|_| invalid())?;
    let contents = toml::to_string(&candidate).map_err(|_| invalid())?;
    let (path, fingerprint) = (writer.path.clone(), writer.expected);
    let saved = tokio::task::spawn_blocking(move || persistence::save(&path, fingerprint, &contents))
        .await.map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Could not save settings."))?
        .map_err(|_| ApiError(StatusCode::CONFLICT, "Could not save settings. Check file permissions or external edits; runtime settings were kept."))?;
    writer.expected = Some(saved);
    admin.hub.lock().unwrap().apply_access(policy);
    tracing::info!(
        revision = candidate.revision,
        "administrator updated access settings"
    );
    Ok(Json(
        json!({"revision":candidate.revision.to_string(), "api_key":key}),
    ))
}
async fn create(State(admin): State<Admin>, Json(input): Json<NewUser>) -> ApiResult {
    change(
        admin,
        input.revision,
        Change::Create(input.name, input.limits),
    )
    .await
}
async fn edit(
    State(admin): State<Admin>,
    Path(name): Path<String>,
    Json(input): Json<EditUser>,
) -> ApiResult {
    change(
        admin,
        input.revision,
        Change::Edit(name, input.enabled, input.limits),
    )
    .await
}
async fn rotate(
    State(admin): State<Admin>,
    Path(name): Path<String>,
    Json(input): Json<Revision>,
) -> ApiResult {
    change(admin, input.revision, Change::Rotate(name)).await
}
async fn delete(
    State(admin): State<Admin>,
    Path(name): Path<String>,
    Json(input): Json<Revision>,
) -> ApiResult {
    change(admin, input.revision, Change::Delete(name)).await
}
async fn access(State(admin): State<Admin>, Json(input): Json<Access>) -> ApiResult {
    change(
        admin,
        input.revision,
        Change::Access(input.require_api_key, input.allowed_sources),
    )
    .await
}
async fn disconnect(State(admin): State<Admin>, Path(id): Path<u64>) -> ApiResult {
    if !admin.hub.lock().unwrap().disconnect(id) {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Subscription already closed.",
        ));
    }
    Ok(Json(json!({"ok":true})))
}
fn router(admin: Admin) -> Router {
    Router::new()
        .route("/api/state", get(state))
        .route("/api/users", post(create))
        .route("/api/users/{name}", put(edit).delete(delete))
        .route("/api/users/{name}/rotate", post(rotate))
        .route("/api/access", put(access))
        .route("/api/subscriptions/{id}/disconnect", post(disconnect))
        .fallback(static_file)
        .layer(DefaultBodyLimit::max(32 * 1024))
        .layer(middleware::from_fn_with_state(admin.clone(), guard))
        .with_state(admin)
}
pub(super) async fn serve(
    cfg: DashboardConfig,
    g: Grpc,
    hub: Hub,
    metrics: Arc<Metrics>,
) -> Result<()> {
    let writer = persistence::Writer::open(cfg.state_file.clone())?;
    let shared = g.shared.as_ref().context("shared mode required")?;
    let managed = load_state(&g, shared)?;
    hub.lock()
        .unwrap()
        .apply_access(AccessPolicy::new(managed)?);
    let admin = Admin {
        hub,
        metrics,
        token: cfg.token()?,
        writer: Arc::new(Mutex::new(writer)),
        assets: Arc::new(assets(&cfg.assets_dir)?),
        info: Arc::new(
            json!({"version":env!("CARGO_PKG_VERSION"),"grpc_listen":g.listen,"commitment":shared.commitment,
            "client_tls":shared.client_tls,"max_connections":g.max_connections,"max_subscriptions":shared.max_subscriptions,
            "max_filters":shared.max_filters,"max_accounts":shared.max_accounts,"allow_unfiltered":shared.allow_unfiltered,
            "max_subscriptions_per_key":shared.max_subscriptions_per_key,"max_filters_per_key":shared.max_filters_per_key,
            "max_accounts_per_key":shared.max_accounts_per_key,"allow_accounts":shared.allow_accounts}),
        ),
        started: Instant::now(),
    };
    let listener = TcpListener::bind(cfg.listen).await?;
    tracing::info!(address=%cfg.listen,"administrator dashboard listening on loopback");
    axum::serve(listener, router(admin)).await?;
    Ok(())
}
