//! Password verification runs on a bounded blocking worker, separate from forwarding.
use super::{persistence, Admin, ApiError, ApiResult, DashboardConfig};
use anyhow::{ensure, Context, Result};
use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const SESSION_LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_SESSIONS: usize = 32;
const ATTEMPTS_PER_MINUTE: u32 = 10;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    username: String,
    password_hash: String,
    must_change_password: bool,
    revision: u64,
}
struct AuthState {
    credentials: Credentials,
    sessions: HashMap<[u8; 32], Instant>,
    fingerprint: [u8; 32],
    attempts: u32,
    window: Instant,
}
pub(super) struct Auth {
    inner: Mutex<AuthState>,
    writer: persistence::Writer,
    work: Arc<Semaphore>,
}
#[derive(Clone, Copy)]
pub(super) struct Session([u8; 32]);

fn username_valid(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "admin" | "administrator" | "root"
        )
}
fn password_valid(value: &str) -> bool {
    value.chars().count() >= 12
        && value.len() <= 128
        && !value.trim().is_empty()
        && !value.chars().any(char::is_control)
}
fn initial(cfg: &DashboardConfig) -> Result<(String, String)> {
    let username = std::env::var(&cfg.initial_username_env).context(
        "run setup-dashboard.sh and start with run.sh to initialize dashboard credentials",
    )?;
    let password = std::env::var(&cfg.initial_password_env)
        .context("dashboard initial password environment variable is missing")?;
    ensure!(username_valid(&username), "dashboard username must be 3..64 ASCII letters/digits/._-; admin, administrator and root are not allowed");
    ensure!(password_valid(&password), "dashboard password must contain at least 12 characters, at most 128 UTF-8 bytes and no control characters");
    Ok((username, password))
}
fn read(cfg: &DashboardConfig) -> Result<Option<(Credentials, [u8; 32])>> {
    persistence::bytes(&cfg.auth_file)?
        .map(|bytes| {
            let text =
                std::str::from_utf8(&bytes).context("invalid dashboard credentials encoding")?;
            let credentials: Credentials = toml::from_str(text).map_err(|_| {
                anyhow::anyhow!(
                    "invalid dashboard credentials file; refusing to reset the password"
                )
            })?;
            ensure!(
                username_valid(&credentials.username),
                "invalid dashboard username"
            );
            let hash = PasswordHash::new(&credentials.password_hash)
                .map_err(|_| anyhow::anyhow!("invalid dashboard password hash"))?;
            // Accept only our bounded Argon2id profile; edited PHC parameters cannot
            // request arbitrary memory/CPU on the password worker.
            ensure!(
                hash.algorithm.as_str() == "argon2id"
                    && hash.version == Some(19)
                    && hash.params.get_decimal("m") == Some(19456)
                    && hash.params.get_decimal("t") == Some(2)
                    && hash.params.get_decimal("p") == Some(1)
                    && hash.params.iter().count() == 3
                    && hash.salt.is_some()
                    && hash.hash.is_some_and(|h| h.len() == 32),
                "unsupported dashboard password hash profile"
            );
            Ok((credentials, Sha256::digest(&bytes).into()))
        })
        .transpose()
}
pub(super) fn validate(cfg: &DashboardConfig) -> Result<()> {
    if read(cfg)?.is_none() {
        initial(cfg)?;
    }
    Ok(())
}
fn hash_password(password: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).context("password salt generation failed")?;
    let salt =
        SaltString::encode_b64(&salt).map_err(|_| anyhow::anyhow!("invalid password salt"))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| anyhow::anyhow!("password hashing failed"))
}
fn verify(password: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}
fn denied() -> ApiError {
    ApiError(StatusCode::UNAUTHORIZED, "账号或密码错误，或登录已过期。")
}
fn busy() -> ApiError {
    ApiError(
        StatusCode::TOO_MANY_REQUESTS,
        "登录或改密请求过于频繁，请稍后重试。",
    )
}
fn failed() -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "无法保存登录信息，请检查文件权限后重试。",
    )
}

impl Auth {
    pub(super) async fn open(cfg: DashboardConfig) -> Result<Arc<Self>> {
        tokio::task::spawn_blocking(move || {
            let mut writer = persistence::Writer::open(cfg.auth_file.clone())?;
            let credentials = match read(&cfg)? {
                Some((credentials, fingerprint)) => {
                    ensure!(writer.expected == Some(fingerprint), "dashboard credentials changed during startup; retry after stopping external edits");
                    credentials
                }
                None => {
                    let (username, password) = initial(&cfg)?;
                    let credentials = Credentials {
                        username,
                        password_hash: hash_password(&password)?,
                        must_change_password: true,
                        revision: 0,
                    };
                    writer.expected = Some(persistence::save(
                        &writer.path,
                        writer.expected,
                        &toml::to_string(&credentials)?,
                    )?);
                    credentials
                }
            };
            Ok(Arc::new(Self {
                inner: Mutex::new(AuthState {
                    credentials,
                    sessions: HashMap::new(),
                    fingerprint: writer.expected.context("dashboard credentials were not persisted")?,
                    attempts: 0,
                    window: Instant::now(),
                }),
                writer,
                work: Arc::new(Semaphore::new(1)),
            }))
        })
        .await?
    }
    fn admit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
        let mut state = self.inner.lock().unwrap();
        if state.window.elapsed() >= Duration::from_secs(60) {
            state.attempts = 0;
            state.window = Instant::now();
        }
        if state.attempts >= ATTEMPTS_PER_MINUTE {
            return Err(busy());
        }
        state.attempts += 1;
        self.work.clone().try_acquire_owned().map_err(|_| busy())
    }
    pub(super) fn authorize(&self, headers: &HeaderMap, path: &str) -> Result<Session, ApiError> {
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let token = values
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(denied)?;
        if values.next().is_some() || token.len() != 64 {
            return Err(denied());
        }
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut state = self.inner.lock().unwrap();
        state
            .sessions
            .retain(|_, expires| *expires > Instant::now());
        if !state.sessions.contains_key(&digest) {
            return Err(denied());
        }
        if state.credentials.must_change_password
            && !matches!(path, "/api/auth/password" | "/api/auth/logout")
        {
            return Err(ApiError(StatusCode::FORBIDDEN, "首次登录请先修改密码。"));
        }
        Ok(Session(digest))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Login {
    username: String,
    password: String,
}
pub(super) async fn login(State(admin): State<Admin>, Json(input): Json<Login>) -> ApiResult {
    let auth = admin.auth;
    let permit = auth.admit()?;
    if input.username.len() > 64 || input.password.len() > 128 {
        return Err(denied());
    }
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let credentials = auth.inner.lock().unwrap().credentials.clone();
        // Verify the password for unknown usernames too, keeping failure timing similar.
        let valid = verify(&input.password, &credentials.password_hash);
        if !valid || input.username != credentials.username {
            return Err(denied());
        }
        let token = super::new_key()?;
        let digest = Sha256::digest(token.as_bytes()).into();
        let mut state = auth.inner.lock().unwrap();
        if state.credentials.revision != credentials.revision {
            return Err(denied());
        }
        state
            .sessions
            .retain(|_, expires| *expires > Instant::now());
        // Refreshes lose memory-only browser tokens. Keep new logins usable
        // without allowing abandoned sessions to grow without bound.
        if state.sessions.len() >= MAX_SESSIONS {
            let oldest = *state
                .sessions
                .iter()
                .min_by_key(|(_, expires)| *expires)
                .unwrap()
                .0;
            state.sessions.remove(&oldest);
        }
        state
            .sessions
            .insert(digest, Instant::now() + SESSION_LIFETIME);
        Ok(Json(json!({
            "token": token,
            "username": credentials.username,
            "must_change_password": credentials.must_change_password,
            "expires_in_seconds": SESSION_LIFETIME.as_secs(),
        })))
    })
    .await
    .map_err(|_| failed())?
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordChange {
    current_password: String,
    new_password: String,
}
pub(super) async fn password(
    State(admin): State<Admin>,
    Extension(session): Extension<Session>,
    Json(input): Json<PasswordChange>,
) -> ApiResult {
    let auth = admin.auth;
    let permit = auth.admit()?;
    if !password_valid(&input.new_password) || input.current_password.len() > 128 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "新密码至少 12 个字符、最多 128 个 UTF-8 字节，且不能包含控制字符。",
        ));
    }
    if input.current_password == input.new_password {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "新密码不能与当前密码相同。",
        ));
    }
    // A started change completes even when its HTTP caller disconnects.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (mut credentials, fingerprint) = {
            let state = auth.inner.lock().unwrap();
            (state.credentials.clone(), state.fingerprint)
        };
        if !verify(&input.current_password, &credentials.password_hash) {
            return Err(ApiError(StatusCode::FORBIDDEN, "当前密码错误。"));
        }
        credentials.password_hash = hash_password(&input.new_password).map_err(|_| failed())?;
        credentials.must_change_password = false;
        credentials.revision = credentials.revision.checked_add(1).ok_or_else(failed)?;
        {
            let state = auth.inner.lock().unwrap();
            if state
                .sessions
                .get(&session.0)
                .is_none_or(|expires| *expires <= Instant::now())
            {
                return Err(denied());
            }
        }
        // No forwarding lock is held; the semaphore serializes password writes.
        let contents = toml::to_string(&credentials).map_err(|_| failed())?;
        let saved = persistence::save(&auth.writer.path, Some(fingerprint), &contents)
            .map_err(|_| failed())?;
        let mut state = auth.inner.lock().unwrap();
        state.credentials = credentials;
        state.fingerprint = saved;
        state.sessions.clear();
        Ok(Json(json!({"ok":true})))
    })
    .await
    .map_err(|_| failed())?
}
pub(super) async fn logout(
    State(admin): State<Admin>,
    Extension(session): Extension<Session>,
) -> ApiResult {
    admin.auth.inner.lock().unwrap().sessions.remove(&session.0);
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
pub(super) fn test_auth(path: std::path::PathBuf, token: &str) -> Arc<Auth> {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let credentials = Credentials {
        username: "sb_test01".into(),
        password_hash: HASH
            .get_or_init(|| hash_password("initial-password-for-tests").unwrap())
            .clone(),
        must_change_password: false,
        revision: 0,
    };
    let mut writer = persistence::Writer::open(path).unwrap();
    let fingerprint =
        persistence::save(&writer.path, None, &toml::to_string(&credentials).unwrap()).unwrap();
    writer.expected = Some(fingerprint);
    Arc::new(Auth {
        inner: Mutex::new(AuthState {
            credentials,
            fingerprint,
            sessions: [(
                Sha256::digest(token.as_bytes()).into(),
                Instant::now() + SESSION_LIFETIME,
            )]
            .into(),
            attempts: 0,
            window: Instant::now(),
        }),
        writer,
        work: Arc::new(Semaphore::new(1)),
    })
}
#[cfg(test)]
mod tests {
    use super::super::{router, tests::Fixture};
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use serde_json::Value;
    use tower::ServiceExt;
    const PASSWORD: &str = "initial-password-for-tests";
    async fn call(
        admin: &Admin,
        path: &str,
        token: Option<&str>,
        input: Value,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some(token) = token {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let response = router(admin.clone())
            .oneshot(req.body(Body::from(input.to_string())).unwrap())
            .await
            .unwrap();
        let code = response.status();
        let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        (code, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }
    async fn login_as(admin: &Admin, password: &str) -> (StatusCode, Value) {
        call(
            admin,
            "/api/auth/login",
            None,
            json!({"username":"sb_test01","password":password}),
        )
        .await
    }
    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }
    #[tokio::test]
    async fn initial_login_password_change_revokes_all_sessions_and_persists() {
        let f = Fixture::new();
        f.admin
            .auth
            .inner
            .lock()
            .unwrap()
            .credentials
            .must_change_password = true;
        assert_eq!(
            login_as(&f.admin, "wrong-password").await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                &f.admin,
                "/api/auth/login",
                None,
                json!({"username":"unknown","password":PASSWORD})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (code, login) = login_as(&f.admin, PASSWORD).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(login["must_change_password"], true);
        let token = login["token"].as_str().unwrap();
        assert!(f
            .admin
            .auth
            .authorize(&headers(token), "/api/state")
            .is_err());
        let (_, other) = login_as(&f.admin, PASSWORD).await;
        let other = other["token"].as_str().unwrap();
        assert_eq!(
            call(
                &f.admin,
                "/api/auth/password",
                Some(token),
                json!({"current_password":"wrong","new_password":"new-password-for-tests"})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(
                &f.admin,
                "/api/auth/password",
                Some(token),
                json!({"current_password":PASSWORD,"new_password":"short"})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &f.admin,
                "/api/auth/password",
                Some(token),
                json!({"current_password":PASSWORD,"new_password":"new-password-for-tests"})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(f
            .admin
            .auth
            .authorize(&headers(token), "/api/state")
            .is_err());
        assert!(f
            .admin
            .auth
            .authorize(&headers(other), "/api/state")
            .is_err());
        assert_eq!(
            login_as(&f.admin, PASSWORD).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (code, login) = login_as(&f.admin, "new-password-for-tests").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(login["must_change_password"], false);
        let token = login["token"].as_str().unwrap();
        assert!(f
            .admin
            .auth
            .authorize(&headers(token), "/api/state")
            .is_ok());
        assert_eq!(
            call(&f.admin, "/api/auth/logout", Some(token), json!({}))
                .await
                .0,
            StatusCode::OK
        );
        assert!(f
            .admin
            .auth
            .authorize(&headers(token), "/api/state")
            .is_err());
        let path = f.admin.auth.writer.path.clone();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            !saved.contains(PASSWORD)
                && !saved.contains("new-password-for-tests")
                && saved.contains("$argon2id$")
        );
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("credentials.toml");
        std::fs::copy(&path, &copy).unwrap();
        let auth = Auth::open(DashboardConfig {
            auth_file: copy,
            ..Default::default()
        })
        .await
        .unwrap();
        let state = auth.inner.lock().unwrap();
        assert!(verify(
            "new-password-for-tests",
            &state.credentials.password_hash
        ));
        assert!(!state.credentials.must_change_password && state.sessions.is_empty());
    }
    #[tokio::test]
    async fn expired_sessions_logout_and_failed_writes_do_not_change_password() {
        let f = Fixture::new();
        let (_, login) = login_as(&f.admin, PASSWORD).await;
        let token = login["token"].as_str().unwrap();
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        f.admin
            .auth
            .inner
            .lock()
            .unwrap()
            .sessions
            .insert(digest, Instant::now() - Duration::from_secs(1));
        assert!(f
            .admin
            .auth
            .authorize(&headers(token), "/api/state")
            .is_err());
        let (_, login) = login_as(&f.admin, PASSWORD).await;
        let token = login["token"].as_str().unwrap();
        std::fs::write(&f.admin.auth.writer.path, "externally changed").unwrap();
        assert_eq!(
            call(
                &f.admin,
                "/api/auth/password",
                Some(token),
                json!({"current_password":PASSWORD,"new_password":"new-password-for-tests"})
            )
            .await
            .0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(f
            .admin
            .auth
            .authorize(&headers(token), "/api/state")
            .is_ok());
        assert!(verify(
            PASSWORD,
            &f.admin.auth.inner.lock().unwrap().credentials.password_hash
        ));
        assert!(read(&DashboardConfig {
            auth_file: f.admin.auth.writer.path.clone(),
            ..Default::default()
        })
        .is_err());
    }
    #[tokio::test]
    async fn login_rate_concurrency_and_session_limits_are_bounded() {
        let f = Fixture::new();
        let permit = f.admin.auth.work.clone().acquire_owned().await.unwrap();
        assert_eq!(
            login_as(&f.admin, PASSWORD).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(permit);
        f.admin.auth.inner.lock().unwrap().attempts = ATTEMPTS_PER_MINUTE;
        assert_eq!(
            login_as(&f.admin, PASSWORD).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
        {
            let mut state = f.admin.auth.inner.lock().unwrap();
            state.window = Instant::now() - Duration::from_secs(61);
            state.sessions = (0..MAX_SESSIONS)
                .map(|n| {
                    (
                        Sha256::digest(n.to_string()).into(),
                        Instant::now() + SESSION_LIFETIME,
                    )
                })
                .collect();
            state.sessions.insert(
                Sha256::digest("0").into(),
                Instant::now() + Duration::from_secs(1),
            );
        }
        assert_eq!(login_as(&f.admin, PASSWORD).await.0, StatusCode::OK);
        let state = f.admin.auth.inner.lock().unwrap();
        assert_eq!(state.sessions.len(), MAX_SESSIONS);
        let oldest: [u8; 32] = Sha256::digest("0").into();
        assert!(!state.sessions.contains_key(&oldest));
    }
    #[test]
    fn password_and_username_rules_and_hash_profile_are_checked() {
        assert!(username_valid("sb_a1b2c3"));
        for name in ["admin", "ADMIN", "administrator", "root", "ab", "a/b"] {
            assert!(!username_valid(name));
        }
        assert!(password_valid("twelve-good-characters"));
        for password in ["short", "            ", "line\nbreak-password"] {
            assert!(!password_valid(password));
        }
        let f = Fixture::new();
        let path = &f.admin.auth.writer.path;
        let text = std::fs::read_to_string(path)
            .unwrap()
            .replace("m=19456", "m=999999999");
        std::fs::write(path, text).unwrap();
        assert!(read(&DashboardConfig {
            auth_file: path.clone(),
            ..Default::default()
        })
        .is_err());
    }
}
