//! Dedicated managed runtime service. There is no project store, legacy launch,
//! arbitrary host command, mount-adoption or network installation endpoint.
#[cfg(target_os = "linux")]
mod managed_delivery_journal;
#[cfg(target_os = "linux")]
mod managed_diagnostics;
#[cfg(target_os = "linux")]
mod managed_http;

#[cfg(all(test, target_os = "linux"))]
mod managed_fixture;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Managed runtime service requires the qualified Linux/own-WSL environment");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
mod native {
    use super::managed_http::{Persistence, Service};
    use anyhow::{Context, Result, ensure};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use cowork_managed_runtime::containerd::bootstrap::{ManagedBootstrap, read_operator_file};
    use cowork_runner_protocol::{Broker, SigningKey};
    use hyper_util::rt::TokioIo;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use serde::Deserialize;
    use std::{
        fs::OpenOptions,
        io::Read,
        net::SocketAddr,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
        path::PathBuf,
        sync::Arc,
        time::Duration,
    };
    use tokio::{net::TcpListener, sync::Semaphore};
    use tokio_rustls::TlsAcceptor;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Configuration {
        format_version: u32,
        listen: SocketAddr,
        bootstrap: ManagedBootstrap,
        state_directory: PathBuf,
        runner_key_file: PathBuf,
        token_file: PathBuf,
        tls_certificate_file: PathBuf,
        tls_key_file: PathBuf,
        result_retention_seconds: u64,
        delivery_journal: Option<DeliveryJournalSetup>,
    }
    #[derive(Deserialize)]
    #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
    enum DeliveryJournalSetup {
        Initialize { slots: usize },
        Open,
    }
    fn pem(bytes: &[u8], label: &str) -> Result<Vec<Vec<u8>>> {
        let text = std::str::from_utf8(bytes)?;
        let begin = format!("-----BEGIN {label}-----");
        let end = format!("-----END {label}-----");
        let mut values = Vec::new();
        let mut remaining = text;
        while !remaining.trim().is_empty() {
            let body = remaining
                .trim_start()
                .strip_prefix(&begin)
                .context("Unexpected TLS PEM data")?;
            let (body, tail) = body.split_once(&end).context("Incomplete TLS PEM data")?;
            let compact = body
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>();
            values.push(STANDARD.decode(compact)?);
            remaining = tail;
            ensure!(values.len() <= 8, "Too many TLS objects");
        }
        ensure!(!values.is_empty(), "Missing TLS object");
        Ok(values)
    }
    fn own_image_sha256() -> Result<String> {
        use ring::digest;
        // Literal kernel-owned original executable, never a request-supplied PID
        // or path. Keep the file open and reject mutable/unlinked binaries.
        let mut file = std::fs::File::open("/proc/self/exe")?;
        let before = file.metadata()?;
        ensure!(
            before.is_file()
                && before.uid() == 0
                && before.nlink() == 1
                && before.mode() & 0o222 == 0
                && before.len() <= 256 * 1024 * 1024,
            "Unpinned service executable"
        );
        let mut hash = digest::Context::new(&digest::SHA256);
        let mut buffer = [0; 65536];
        let mut total = 0u64;
        loop {
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            total += size as u64;
            ensure!(total <= 256 * 1024 * 1024, "Service executable limit");
            hash.update(&buffer[..size]);
        }
        let after = file.metadata()?;
        ensure!(
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.len() == after.len()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec()
                && total == before.len(),
            "Service executable changed"
        );
        Ok(hash
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    pub async fn run() -> Result<()> {
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "Runtime management requires its dedicated root service"
        );
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        ensure!(
            args.len() == 2 && args[0] == "--config",
            "Usage: cowork-managed-runtime-service --config <operator-file>"
        );
        let configuration: Configuration =
            serde_json::from_slice(&read_operator_file(&PathBuf::from(&args[1]), 65536, false)?)?;
        ensure!(
            configuration.format_version == 2,
            "Runtime service configuration upgrade required"
        );
        ensure!(
            configuration.result_retention_seconds > 0
                && configuration.result_retention_seconds <= 7 * 24 * 60 * 60,
            "Invalid output retention"
        );
        let installed = configuration.bootstrap.installed()?;
        ensure!(
            own_image_sha256()? == installed.binding.adapter_build_sha256,
            "Installed runtime build does not match the actual service executable"
        );
        let directory = configuration
            .bootstrap
            .state_directory(&configuration.state_directory)?;
        let mut options = cap_std::fs::OpenOptions::new();
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::fs::OpenOptionsExt;
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .follow(FollowSymlinks::No);
        let lock = directory.open_with("service.lock", &options)?.into_std();
        let metadata = lock.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == 0
                && metadata.nlink() == 1
                && metadata.mode() & 0o077 == 0,
            "Invalid service lock"
        );
        lock.try_lock()
            .context("Another runtime service owns this state")?;
        let signer = SigningKey::from_pkcs8(&read_operator_file(
            &configuration.runner_key_file,
            16384,
            true,
        )?)?;
        ensure!(
            signer.public_key() == installed.binding.runner_public_key,
            "Runtime signing identity changed"
        );
        let token = String::from_utf8(read_operator_file(&configuration.token_file, 16384, true)?)?
            .trim()
            .to_owned();
        let database = configuration.state_directory.join("runner.db");
        let journal_setup = configuration
            .delivery_journal
            .context("Explicit original delivery journal setup required")?;
        let initialize = matches!(journal_setup, DeliveryJournalSetup::Initialize { .. });
        if initialize {
            ensure!(
                !database.exists(),
                "Journal initialization requires a new original runtime database"
            );
        } else {
            ensure!(
                database.exists(),
                "Original runtime database missing; recovery cannot initialize it"
            );
        }
        if !database.exists() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&database)?;
            file.sync_all()?;
            directory.open(".")?.into_std().sync_all()?;
        }
        let metadata = std::fs::symlink_metadata(&database)?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == 0
                && metadata.nlink() == 1
                && metadata.mode() & 0o077 == 0,
            "Invalid private runtime database"
        );
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect(&format!(
                "sqlite://{}?mode=rw",
                database
                    .to_str()
                    .context("Runtime database path encoding")?
            ))
            .await?;
        for statement in [
            "PRAGMA journal_mode=WAL",
            "PRAGMA synchronous=FULL",
            "PRAGMA busy_timeout=5000",
            "PRAGMA temp_store=MEMORY",
            "PRAGMA journal_size_limit=10485760",
        ] {
            sqlx::query(statement).execute(&pool).await?;
        }
        if !initialize {
            let row: (String, i64) =
                sqlx::query_as("SELECT payload,high_water FROM runner_binding WHERE id=1")
                    .fetch_one(&pool)
                    .await
                    .context("Original runtime binding unavailable")?;
            ensure!(
                row.0 == serde_json::to_string(&installed.binding)? && row.1 >= 0,
                "Original runtime database binding changed"
            );
        }
        let broker = Broker::open(pool, installed.binding.clone(), signer.clone()).await?;
        let journal = match journal_setup {
            DeliveryJournalSetup::Initialize { slots } => {
                super::managed_delivery_journal::Journal::initialize(
                    &directory,
                    &installed.binding,
                    slots,
                )?
            }
            DeliveryJournalSetup::Open => {
                super::managed_delivery_journal::Journal::open(&directory, &installed.binding)?
            }
        };
        let diagnostics = super::managed_diagnostics::Diagnostics::open(&directory)?;
        let history_only = !initialize && !journal.can_start_new_receiver().unwrap_or(false);
        let backend = if history_only || journal.can_reserve().is_err() {
            None
        } else {
            match configuration.bootstrap.activate(broker.clone()) {
                Ok(active) => Some(active),
                Err(error) if !initialize => {
                    let _ = diagnostics.record(
                        "managed_runtime_recovery_only",
                        "startup",
                        &error,
                        serde_json::Value::Null,
                    );
                    None
                }
                Err(error) => return Err(error.into()),
            }
        };
        let service = Service::new(
            backend,
            broker,
            signer,
            installed,
            &token,
            i64::try_from(configuration.result_retention_seconds)?
                .checked_mul(1000)
                .context("Retention overflow")?,
            Persistence {
                diagnostics,
                journal,
            },
        )?;
        let certificates = pem(
            &read_operator_file(&configuration.tls_certificate_file, 1024 * 1024, false)?,
            "CERTIFICATE",
        )?
        .into_iter()
        .map(CertificateDer::from)
        .collect();
        let keys = pem(
            &read_operator_file(&configuration.tls_key_file, 16384, true)?,
            "PRIVATE KEY",
        )?;
        ensure!(keys.len() == 1, "Exactly one PKCS8 TLS key is required");
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                certificates,
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    keys.into_iter().next().context("TLS key missing")?,
                )),
            )?;
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind(configuration.listen).await?;
        let connections = Arc::new(Semaphore::new(16));
        let router = service.router();
        // A shutdown drops only this server process; independent native fences
        // and durable reservations remain responsible for unconfirmed jobs.
        loop {
            tokio::select! {
                result=tokio::signal::ctrl_c()=>{result?;break;},
                result=listener.accept()=>{
                    let (socket,_)=result?;let Ok(permit)=connections.clone().try_acquire_owned()else{continue;};
                    let acceptor=acceptor.clone();let router=router.clone();
                    tokio::spawn(async move {
                        let _permit=permit;
                        let Ok(Ok(socket))=tokio::time::timeout(Duration::from_secs(10),acceptor.accept(socket)).await else{return;};
                        let handler=hyper::service::service_fn(move|request|{let router=router.clone();async move{router.oneshot(request).await}});
                        let _=tokio::time::timeout(Duration::from_secs(120),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),handler)).await;
                    });
                },
            }
        }
        drop(listener);
        drop(lock);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    native::run().await
}
