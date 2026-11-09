use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs::File,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};
#[cfg(windows)]
#[path = "../../daemon/src/private_secret_file.rs"]
#[allow(dead_code)]
mod private_secret_file;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub format: u32,
    pub listen: SocketAddr,
    pub preview_domain: String,
    pub app_origin: String,
    pub namespace: String,
    pub service_suffix: String,
    pub allowed_ports: Vec<u16>,
    pub runner_id: String,
    pub runner_generation: u64,
    pub runner_public_key: Vec<u8>,
    pub runner_url: String,
    pub runner_bearer_file: PathBuf,
    pub runner_ca_file: Option<PathBuf>,
    pub max_ticket_ms: i64,
}

pub fn dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}
fn dns(value: &str) -> bool {
    value.len() <= 253 && value.split('.').count() >= 2 && value.split('.').all(dns_label)
}
fn site(value: &str) -> String {
    value.rsplit('.').take(2).collect::<Vec<_>>().join(".")
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format == 1 && dns(&self.preview_domain) && dns_label(&self.namespace),
            "Invalid preview domain or namespace"
        );
        let app = reqwest::Url::parse(&self.app_origin)?;
        ensure!(
            app.scheme() == "https"
                && app.username().is_empty()
                && app.password().is_none()
                && app.query().is_none()
                && app.fragment().is_none()
                && matches!(app.path(), "" | "/")
                && app
                    .host_str()
                    .is_some_and(|host| dns(host) && site(host) != site(&self.preview_domain)),
            "App and preview must use separate base domains over HTTPS"
        );
        ensure!(
            self.service_suffix == format!(".{}.svc.cluster.local", self.namespace),
            "Service DNS suffix must identify the fixed workload namespace"
        );
        ensure!(
            !self.allowed_ports.is_empty()
                && self.allowed_ports.len() <= 16
                && self
                    .allowed_ports
                    .iter()
                    .all(|port| *port >= 1024 && !matches!(*port, 8080 | 9090))
                && self
                    .allowed_ports
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == self.allowed_ports.len(),
            "Preview ports must be explicitly configured and exclude sandbox control ports"
        );
        ensure!(
            !self.runner_id.is_empty()
                && self.runner_id.len() <= 128
                && self.runner_generation > 0
                && self.runner_public_key.len() == 32
                && (1000..=60_000).contains(&self.max_ticket_ms),
            "Invalid pinned runner authority"
        );
        let runner = reqwest::Url::parse(&self.runner_url)?;
        ensure!(
            runner.scheme() == "https"
                && runner.host_str().is_some()
                && runner.username().is_empty()
                && runner.password().is_none()
                && runner.query().is_none()
                && runner.fragment().is_none()
                && matches!(runner.path(), "" | "/"),
            "Runner validator requires a pinned HTTPS origin"
        );
        Ok(())
    }
}
fn bytes(path: &Path, max: u64, private: bool) -> Result<Vec<u8>> {
    ensure!(path.is_absolute(), "Select absolute configuration paths");
    for part in path.ancestors() {
        let metadata = std::fs::symlink_metadata(part)?;
        ensure!(!metadata.is_symlink(), "Aliased configuration path");
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "Reparse configuration path"
            );
        }
    }
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= max,
        "Configuration size or type invalid"
    );
    if private {
        #[cfg(windows)]
        private_secret_file::validate(&file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                metadata.mode() & 0o077 == 0
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.nlink() == 1,
                "Private owner-only configuration required"
            );
        }
    }
    let mut output = Vec::new();
    file.by_ref().take(max + 1).read_to_end(&mut output)?;
    ensure!(output.len() as u64 <= max, "Configuration too large");
    Ok(output)
}
pub fn load(path: &Path) -> Result<(Config, String, reqwest::Client)> {
    let config: Config = serde_json::from_slice(&bytes(path, 64 * 1024, true)?)
        .context("Invalid preview configuration")?;
    config.validate()?;
    let raw = bytes(&config.runner_bearer_file, 8192, true)?;
    let token = std::str::from_utf8(&raw)?.trim().to_owned();
    ensure!(
        !token.is_empty() && token.bytes().all(|byte| (33..=126).contains(&byte)),
        "Invalid runner credential"
    );
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30));
    if let Some(path) = &config.runner_ca_file {
        client = client.add_root_certificate(reqwest::Certificate::from_pem(&bytes(
            path,
            1024 * 1024,
            false,
        )?)?);
    }
    Ok((config, token, client.build()?))
}
