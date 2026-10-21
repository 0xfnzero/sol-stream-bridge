//! Exercise credential bootstrap through the shipped CLI in an isolated process.
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn bootstrap_check_is_read_only_and_saved_credentials_take_precedence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/index.html"), "<html></html>").unwrap();
    // Reserve three distinct ports while generating config, then release them.
    let ports: Vec<_> = (0..3)
        .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let config = format!(
        r#"role = "receiver"
[grpc]
tunnel_address = "{}"
listen = "{}"
allowed_sources = ["127.0.0.1"]
token_env = "DASHBOARD_TEST_BRIDGE_TOKEN"
insecure = true
[grpc.shared]
[grpc.shared.dashboard]
listen = "{}"
auth_file = "admin.toml"
state_file = "state.toml"
assets_dir = "assets"
"#,
        ports[0].local_addr().unwrap(),
        ports[1].local_addr().unwrap(),
        ports[2].local_addr().unwrap(),
    );
    std::fs::write(dir.path().join("config.toml"), config).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stream-bridge-server"));
    command
        .current_dir(dir.path())
        .args(["--config", "config.toml"])
        .env(
            "DASHBOARD_TEST_BRIDGE_TOKEN",
            "test-bridge-token-at-least-32-characters",
        )
        .env("DASHBOARD_INITIAL_USERNAME", "sb_test01")
        .env("DASHBOARD_INITIAL_PASSWORD", "bootstrap-password-for-tests");
    let checked = command.arg("--check").output().unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(!dir.path().join("admin.toml").exists());
    assert!(!dir.path().join("admin.toml.lock").exists());

    let mut start = Command::new(command.get_program());
    start
        .current_dir(dir.path())
        .args(["--config", "config.toml"]);
    for (key, value) in command.get_envs() {
        start.env(key, value.unwrap());
    }
    drop(ports);
    let log = std::fs::File::create(dir.path().join("server.log")).unwrap();
    start
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log));
    let mut server = Server(start.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let saved = loop {
        if let Ok(saved) = std::fs::read_to_string(dir.path().join("admin.toml")) {
            break saved;
        }
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited: {}",
            std::fs::read_to_string(dir.path().join("server.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "credential bootstrap timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(server);
    let credentials: toml::Value = toml::from_str(&saved).unwrap();
    assert_eq!(credentials["username"].as_str(), Some("sb_test01"));
    assert_eq!(credentials["must_change_password"].as_bool(), Some(true));
    assert!(!saved.contains("bootstrap-password-for-tests"));
    let hash = PasswordHash::new(credentials["password_hash"].as_str().unwrap()).unwrap();
    Argon2::default()
        .verify_password(b"bootstrap-password-for-tests", &hash)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("admin.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    // Even invalid bootstrap values cannot reset an established account.
    command
        .env("DASHBOARD_INITIAL_USERNAME", "admin")
        .env("DASHBOARD_INITIAL_PASSWORD", "short");
    assert!(command.output().unwrap().status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("admin.toml")).unwrap(),
        saved
    );
    // Corruption fails closed even though valid bootstrap credentials remain available.
    std::fs::write(dir.path().join("admin.toml"), "broken credentials").unwrap();
    command
        .env("DASHBOARD_INITIAL_USERNAME", "sb_test01")
        .env("DASHBOARD_INITIAL_PASSWORD", "bootstrap-password-for-tests");
    assert!(!command.output().unwrap().status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("admin.toml")).unwrap(),
        "broken credentials"
    );
}
