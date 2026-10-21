pub use stream_bridge_client as bot;
pub mod config;
pub mod tunnel;
pub mod udp;

pub async fn run(config: config::Config) -> anyhow::Result<()> {
    config.validate()?;
    let mut tasks = tokio::task::JoinSet::new();
    for route in config.shred {
        tasks.spawn(udp::run(route));
    }
    if let Some(g) = config.grpc {
        tasks.spawn(async move {
            if config.role == config::Role::Source {
                tunnel::source(g).await
            } else {
                tunnel::receiver(g).await
            }
        });
    }
    match tasks.join_next().await {
        Some(result) => result??,
        None => anyhow::bail!("no bridge tasks configured"),
    }
    anyhow::bail!("bridge task exited unexpectedly")
}
