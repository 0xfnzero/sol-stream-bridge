use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use stream_bridge_server::{
    config::{Config, Role},
    tunnel,
};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
    #[arg(long)]
    check: bool,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    if let Some(g) = &config.grpc {
        tunnel::validate_tls(g, config.role == Role::Source)?;
    }
    if args.check {
        tracing::info!("configuration valid");
        return Ok(());
    }
    tokio::select! {
        result = stream_bridge_server::run(config) => result,
        result = shutdown() => { result?; tracing::info!("shutting down"); Ok(()) }
    }
}
async fn shutdown() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { r = tokio::signal::ctrl_c() => r?, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
