//! Run with: cargo run --example receive_client --features yellowstone -- client.toml
mod common;

#[path = "common/standalone.rs"]
mod standalone;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    standalone::run("client.toml").await
}
