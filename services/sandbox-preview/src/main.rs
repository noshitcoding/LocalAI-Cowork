mod config;
mod gateway;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let path = match args.as_slice() {
        [flag, path] if flag == "--config" => std::path::PathBuf::from(path),
        _ => anyhow::bail!("Usage: cowork-sandbox-preview --config ABS/config.json"),
    };
    let (config, token, client) = config::load(&path)?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .context("Bind preview gateway")?;
    let gateway = gateway::Gateway::new(config, token, client)?;
    axum::serve(listener, gateway.router())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
