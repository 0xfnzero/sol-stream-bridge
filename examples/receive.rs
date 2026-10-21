//! Run with: cargo run --example receive --features yellowstone -- clients/client.toml
#[path = "../clients/rust/examples/common/mod.rs"]
mod common;

#[path = "../clients/rust/examples/common/standalone.rs"]
mod standalone;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    standalone::run("clients/client.toml").await
}
