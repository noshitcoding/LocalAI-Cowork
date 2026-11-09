//! Immutable remote policy and separately retained actual native path custody.
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use cowork_contracts::enterprise::{safe_relative_path, LaptopAccess};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub access: LaptopAccess,
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub capabilities: BTreeSet<String>,
    pub offline_seconds: u32,
    pub max_offline_events: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub grant_id: Uuid,
    pub revision: i64,
    pub policy_sha256: String,
    pub access: LaptopAccess,
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub capabilities: BTreeSet<String>,
    pub offline_seconds: u32,
    pub max_offline_events: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Custody {
    pub format: u16,
    pub server_url: String,
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub pending_revision: i64,
    pub policy_sha256: String,
    pub original_grant: Value,
    pub policy: Policy,
    pub roots: BTreeMap<String, PathBuf>,
    pub approved_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub sandbox: Option<Value>,
}
pub fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        if matches!(self.access, LaptopAccess::None)
            || self.read_roots.len() > 64
            || self.write_roots.len() > 64
            || self
                .read_roots
                .iter()
                .chain(&self.write_roots)
                .any(|p| !safe_relative_path(p) || p.len() > 1024)
            || self.capabilities.len() > 128
            || self
                .capabilities
                .iter()
                .any(|c| c.is_empty() || c.len() > 128)
            || self.offline_seconds > 3600
            || !(1..=10000).contains(&self.max_offline_events)
        {
            bail!("enterprise policy bounds invalid")
        }
        Ok(())
    }
}
impl Pin {
    pub fn from_input(input: &Value) -> Result<Option<Self>> {
        input
            .get("_enterprise_grant")
            .map(|v| {
                serde_json::from_value(v.clone()).context("invalid original enterprise grant pin")
            })
            .transpose()
    }
    pub fn policy(&self) -> Policy {
        Policy {
            access: self.access.clone(),
            read_roots: self.read_roots.clone(),
            write_roots: self.write_roots.clone(),
            capabilities: self.capabilities.clone(),
            offline_seconds: self.offline_seconds,
            max_offline_events: self.max_offline_events,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.policy().validate()?;
        if self.grant_id.is_nil()
            || self.revision < 2
            || !sha(&self.policy_sha256)
            || digest(&self.policy())? != self.policy_sha256
        {
            bail!("enterprise policy hash differs from original pin")
        }
        Ok(())
    }
}
impl Custody {
    pub fn from_original(
        server_url: &str,
        device_id: Uuid,
        original: Value,
        roots: BTreeMap<String, PathBuf>,
        sandbox: Option<Value>,
    ) -> Result<Self> {
        let grant_id: Uuid = serde_json::from_value(original["id"].clone())?;
        let actual: Uuid = serde_json::from_value(original["device_id"].clone())?;
        let revision = original["revision"]
            .as_i64()
            .context("grant revision missing")?;
        let policy: Policy = serde_json::from_value(original["policy"].clone())?;
        policy.validate()?;
        let policy_sha256 = original["policy_sha256"]
            .as_str()
            .context("grant policy hash missing")?
            .to_owned();
        let expires_at: DateTime<Utc> = serde_json::from_value(original["expires_at"].clone())?;
        if actual != device_id
            || original["current_authorized"] != true
            || grant_id.is_nil()
            || revision < 1
            || !sha(&policy_sha256)
            || digest(&policy)? != policy_sha256
            || expires_at <= Utc::now()
            || original["state"] != "pending"
        {
            bail!("current original pending device grant required")
        }
        for alias in policy.read_roots.iter().chain(&policy.write_roots) {
            let native = roots
                .get(alias)
                .context("native root alias was not selected locally")?;
            known_root(native)?;
        }
        if roots
            .keys()
            .any(|alias| !policy.read_roots.contains(alias) && !policy.write_roots.contains(alias))
        {
            bail!("native custody has unrequested roots")
        }
        if matches!(policy.access, LaptopAccess::Selected) && roots.is_empty() {
            bail!("selected mode requires actual native roots")
        }
        if matches!(policy.access, LaptopAccess::Sandbox) && sandbox.is_none() {
            bail!("sandbox requires actual local OCI qualification")
        }
        Ok(Self {
            format: 1,
            server_url: server_url.trim_end_matches('/').into(),
            device_id,
            grant_id,
            pending_revision: revision,
            policy_sha256,
            original_grant: original,
            policy,
            roots,
            approved_at: Utc::now(),
            expires_at,
            sandbox,
        })
    }
    pub fn validate(&self, server: &str, device: Uuid, pin: &Pin) -> Result<()> {
        pin.validate()?;
        if self.format != 1
            || self.server_url != server.trim_end_matches('/')
            || self.device_id != device
            || self.grant_id != pin.grant_id
            || self.pending_revision.checked_add(1) != Some(pin.revision)
            || self.policy_sha256 != pin.policy_sha256
            || digest(&self.policy)? != pin.policy_sha256
            || self.expires_at <= Utc::now()
        {
            bail!("original native enterprise custody expired or changed")
        };
        for root in self.roots.values() {
            known_root(root)?;
        }
        Ok(())
    }
    pub fn confirmation(&self) -> Result<Value> {
        Ok(
            json!({"expected_revision":self.pending_revision,"policy_sha256":self.policy_sha256,"local_custody_sha256":digest(self)?}),
        )
    }
}
pub fn known_root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("native selected root must be absolute")
    };
    let mut current = PathBuf::new();
    for c in path.components() {
        current.push(c.as_os_str());
        if matches!(c, std::path::Component::Prefix(_)) {
            continue;
        }
        let meta =
            std::fs::symlink_metadata(&current).context("native root is not locally present")?;
        if indirect(&meta) {
            bail!("native custody refuses symlink roots")
        }
    }
    let canonical = path.canonicalize()?;
    if !canonical.is_dir() {
        bail!("native root must be a directory")
    };
    Ok(canonical)
}
pub fn indirect(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}
