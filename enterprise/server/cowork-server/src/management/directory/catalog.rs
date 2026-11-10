//! Local operator material. HTTP callers never select URLs, bind credentials or files.
use super::*;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ldap {
    pub url: String,
    pub base_dn: String,
    pub bind_dn: String,
    pub users_filter: String,
    pub groups_filter: String,
    pub subject_attribute: String,
    pub username_attribute: String,
    pub group_name_attribute: String,
    pub member_attribute: String,
    pub disabled_attribute: Option<String>,
    pub disabled_value: Option<String>,
    pub ca_file: Option<String>,
    pub page_size: u32,
    pub poll_seconds: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEntry {
    id: Uuid,
    organisation_id: Uuid,
    provider_id: String,
    issuer: String,
    client_id: String,
    kind: String,
    credential_file: String,
    ldap: Option<Ldap>,
}
pub struct Connector {
    pub id: Uuid,
    pub org: Uuid,
    pub provider: String,
    pub issuer: String,
    pub client: String,
    pub kind: String,
    pub fingerprint: String,
    pub credential: String,
    pub ldap: Option<Ldap>,
    pub ca_certificates: Option<Vec<Vec<u8>>>,
}
pub type Registry = HashMap<Uuid, Arc<Connector>>;
pub fn private_file(path: &str, max: u64) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let canonical = Path::new(path).canonicalize()?;
    let metadata = std::fs::metadata(&canonical)?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= max,
        "invalid directory operator file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o077 == 0 && metadata.nlink() == 1,
            "directory operator files must be private regular files"
        );
        let parent = std::fs::metadata(
            canonical
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing private directory"))?,
        )?;
        anyhow::ensure!(
            parent.mode() & 0o022 == 0,
            "directory operator parent must not be writable by others"
        );
    }
    let mut bytes = Vec::new();
    std::fs::File::open(canonical)?
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= max,
        "directory operator file exceeded bound"
    );
    Ok(bytes)
}
pub fn load(management: bool) -> anyhow::Result<Registry> {
    if !management {
        return Ok(HashMap::new());
    }
    let path = match std::env::var("COWORK_ORGANISATION_DIRECTORY_FILE") {
        Ok(p) => p,
        Err(std::env::VarError::NotPresent) => return Ok(HashMap::new()),
        Err(e) => return Err(e.into()),
    };
    let entries: Vec<FileEntry> = serde_json::from_slice(&private_file(&path, 1_048_576)?)?;
    anyhow::ensure!(entries.len() <= 256, "too many directory connectors");
    let mut result = HashMap::new();
    for e in entries {
        anyhow::ensure!(
            !e.id.is_nil() && !e.organisation_id.is_nil(),
            "directory IDs must be original nonnil UUIDs"
        );
        anyhow::ensure!(
            matches!(e.kind.as_str(), "scim" | "ldap") && (e.kind == "ldap") == e.ldap.is_some(),
            "invalid directory connector kind"
        );
        anyhow::ensure!(
            !e.provider_id.is_empty()
                && e.provider_id.len() <= 64
                && e.provider_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
            "invalid directory provider"
        );
        let issuer = crate::oidc::validated_https_or_loopback_url(&e.issuer, "directory issuer")?;
        anyhow::ensure!(
            issuer.query().is_none()
                && issuer.fragment().is_none()
                && !e.client_id.is_empty()
                && e.client_id.len() <= 512,
            "invalid directory provider identity"
        );
        let credential = String::from_utf8(private_file(&e.credential_file, 16_384)?)?
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        anyhow::ensure!(
            !credential.is_empty() && credential.len() <= 8192 && !credential.contains('\0'),
            "invalid directory credential"
        );
        if e.kind == "scim" {
            anyhow::ensure!(
                credential.len() >= 32 && credential.bytes().all(|b| (0x21..=0x7e).contains(&b)),
                "SCIM bearer needs at least32 printable ASCII bytes"
            );
        }
        if let Some(l) = &e.ldap {
            let url = url_check(&l.url)?;
            anyhow::ensure!(
                url && !l.base_dn.is_empty()
                    && l.base_dn.len() <= 4096
                    && !l.bind_dn.is_empty()
                    && l.bind_dn.len() <= 4096,
                "invalid LDAP endpoint or DN"
            );
            anyhow::ensure!(
                (1..=500).contains(&l.page_size) && (30..=86_400).contains(&l.poll_seconds),
                "invalid LDAP page/poll bound"
            );
            for f in [&l.users_filter, &l.groups_filter] {
                anyhow::ensure!(
                    f.starts_with('(') && f.ends_with(')') && f.len() <= 4096 && !f.contains('\0'),
                    "invalid LDAP filter"
                );
            }
            for a in [
                &l.subject_attribute,
                &l.username_attribute,
                &l.group_name_attribute,
                &l.member_attribute,
            ]
            .into_iter()
            .chain(l.disabled_attribute.as_ref())
            {
                anyhow::ensure!(
                    !a.is_empty()
                        && a.len() <= 120
                        && a.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-;".contains(&b)),
                    "invalid LDAP attribute"
                );
            }
            anyhow::ensure!(
                l.disabled_attribute.is_some() == l.disabled_value.is_some(),
                "disabled attribute/value must be paired"
            );
            if let Some(p) = &l.ca_file {
                let _ = private_file(p, 1_048_576)?;
            }
        }
        let ca_bytes = e
            .ldap
            .as_ref()
            .and_then(|l| l.ca_file.as_ref())
            .map(|p| private_file(p, 1_048_576))
            .transpose()?;
        let ca_certificates = ca_bytes
            .as_ref()
            .map(|bytes| -> anyhow::Result<Vec<Vec<u8>>> {
                let parsed = openssl::x509::X509::stack_from_pem(bytes)?;
                anyhow::ensure!(!parsed.is_empty(), "LDAP CA file must contain certificates");
                parsed
                    .iter()
                    .map(|c| c.to_der().map_err(Into::into))
                    .collect()
            })
            .transpose()?;
        let fingerprint = sha(&json!([
            1,
            e.id,
            e.organisation_id,
            e.provider_id,
            e.issuer,
            e.client_id,
            e.kind,
            e.ldap,
            hex::encode(Sha256::digest(credential.as_bytes())),
            ca_bytes.as_ref().map(|b| hex::encode(Sha256::digest(b)))
        ]));
        let c = Connector {
            id: e.id,
            org: e.organisation_id,
            provider: e.provider_id,
            issuer: e.issuer,
            client: e.client_id,
            kind: e.kind,
            fingerprint,
            credential,
            ldap: e.ldap,
            ca_certificates,
        };
        anyhow::ensure!(
            result.insert(c.id, Arc::new(c)).is_none(),
            "duplicate directory connector"
        );
    }
    Ok(result)
}
fn url_check(raw: &str) -> anyhow::Result<bool> {
    let u = reqwest::Url::parse(raw)?;
    Ok(u.scheme() == "ldaps"
        && u.host_str().is_some()
        && u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none()
        && matches!(u.path(), "" | "/")
        && raw == raw.trim())
}
