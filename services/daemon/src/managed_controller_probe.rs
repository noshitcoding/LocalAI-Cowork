//! Private architecture probe using the actual Store, worker and daemon HTTPS
//! adapter. The loopback model is a declared deterministic proposal fixture.
//! This does not qualify a real model, desktop UI or a released runtime profile.
#[path = "../examples/managed_controller_qa/exercise.rs"]
mod exercise;
#[path = "../examples/managed_controller_qa/prepare.rs"]
mod prepare;
#[cfg(windows)]
#[allow(dead_code)]
mod private_secret_file;
#[allow(dead_code)]
mod process_runner_config;

use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
    let mut args = std::env::args_os().skip(1);
    let phase = args.next().context("Expected prepare or exercise")?;
    ensure!(
        args.next().as_deref() == Some(std::ffi::OsStr::new("--fixture-root")),
        "Explicit fixture root required"
    );
    let root = PathBuf::from(args.next().context("Missing fixture root")?);
    match phase.to_str() {
        Some("prepare") => {
            ensure!(args.next().is_none(), "Unexpected prepare arguments");
            prepare::run(&root).await
        }
        Some("exercise") => {
            ensure!(
                args.next().as_deref() == Some(std::ffi::OsStr::new("--service-handoff")),
                "Explicit service handoff required"
            );
            let handoff = PathBuf::from(args.next().context("Missing service handoff")?);
            ensure!(args.next().is_none(), "Unexpected exercise arguments");
            tokio::time::timeout(
                std::time::Duration::from_secs(150),
                Box::pin(exercise::run(&root, &handoff)),
            )
            .await
            .context("Controller exercise deadline elapsed")?
        }
        _ => anyhow::bail!("Expected prepare or exercise"),
    }
}
