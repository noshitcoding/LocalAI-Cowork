//! Device-owned OS key custody and immutable original MCP IO records.
use super::*;
use cowork_secrets::{
    KeyId, KeyProof, McpSecretScope, NativeKeyManager, NativeKeyStore, SealedMcpBlob,
    open_mcp_blob, prove_key, seal_mcp_blob, verify_key,
};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    format: String,
    scope: McpSecretScope,
    sealed: SealedMcpBlob,
}
pub(super) struct Journal {
    directory: PathBuf,
    keys: NativeKeyStore,
    key: KeyId,
    run: Uuid,
    source: Uuid,
    binding: String,
    server: String,
    project: String,
}
fn create(path: &Path) -> Result<fs::File> {
    crate::create_private(path)
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut f = create(path)?;
    f.write_all(&serde_json::to_vec(value)?)?;
    f.sync_all()?;
    #[cfg(unix)]
    fs::File::open(path.parent().context("device_mcp_journal_parent")?)?.sync_all()?;
    Ok(())
}
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    // Sealed ciphertext is a JSON byte vector. The bounded four-MiB original
    // response can expand to nearly sixteen MiB without changing its scope.
    serde_json::from_reader(crate::config::private_file(path, 20 * 1024 * 1024)?)
        .context("device_mcp_original_journal_invalid")
}
fn custody(directory: &Path) -> Result<(NativeKeyStore, KeyId)> {
    crate::new_directory(directory)?;
    let locator = directory.join("key-candidate.json");
    let proofpath = directory.join("key-proof.json");
    let fresh = if locator.exists() {
        false
    } else {
        let id = KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        };
        write(&locator, &id)?;
        true
    };
    let id: KeyId = read(&locator)?;
    id.validate()
        .map_err(|_| anyhow::anyhow!("device_mcp_original_key_invalid"))?;
    let keys = NativeKeyStore::open(id.instance)
        .map_err(|_| anyhow::anyhow!("device_mcp_os_keystore_unavailable"))?;
    if fresh {
        keys.create(&id).map_err(|_| {
            anyhow::anyhow!("device_mcp_key_setup_unconfirmed_use_explicit_confirm")
        })?;
        let proof = prove_key(&keys, &id)
            .map_err(|_| anyhow::anyhow!("device_mcp_original_key_unconfirmed"))?;
        write(&proofpath, &proof)?;
    }
    let proof: KeyProof =
        read(&proofpath).context("device_mcp_key_setup_unconfirmed_use_explicit_confirm")?;
    ensure!(proof.key == id, "device_mcp_original_key_differs");
    verify_key(&keys, &proof)
        .map_err(|_| anyhow::anyhow!("device_mcp_original_key_unavailable_no_rekey"))?;
    Ok((keys, id))
}
impl Journal {
    pub fn open(
        engine: &ExecutionEngine,
        run: Uuid,
        source: Uuid,
        binding: &McpBinding,
    ) -> Result<Self> {
        let directory = engine.config.data_directory.join("mcp-journal");
        let (keys, key) = custody(&directory)?;
        let configured = json!({"name":binding.name,"transport":match binding.transport{McpTransport::Stdio=>"stdio",McpTransport::StreamableHttp=>"streamable_http"},"endpoint":binding.endpoint,"bearer":binding.bearer,"command":binding.command,"arguments":binding.arguments,"environment":binding.environment,"catalog":binding.catalog_sha256,"tools":binding.allowed_tools});
        Ok(Self {
            directory,
            keys,
            key,
            run,
            source,
            binding: digest(&serde_json::to_vec(&configured)?),
            server: binding.name.clone(),
            project: super::interactions::project().unwrap_or(run).to_string(),
        })
    }
    fn scope(&self, key: &str, purpose: &str) -> McpSecretScope {
        McpSecretScope {
            owner_kind: "native".into(),
            owner_id: self.key.instance.to_string(),
            project_id: self.project.clone(),
            server_id: self.server.clone(),
            record_id: digest(
                &serde_json::to_vec(&(self.run, self.source, key)).expect("plain record identity"),
            ),
            revision: 1,
            epoch: self.source,
            configuration_sha256: self.binding.clone(),
            purpose: purpose.into(),
        }
    }
    pub fn put(&self, key: &str, purpose: &str, value: &Value) -> Result<String> {
        let scope = self.scope(key, purpose);
        let path = self.directory.join(format!(
            "{}-{}-{}.sealed.json",
            self.run, self.source, scope.record_id
        ));
        let payload = json!({"format":"device-mcp-original/1","run":self.run,"source":self.source,"original_binding_sha256":self.binding,"operation_key":key,"value":value});
        let sealed = seal_mcp_blob(&self.keys, self.key, &scope, &serde_json::to_vec(&payload)?)
            .map_err(|_| anyhow::anyhow!("device_mcp_original_seal_failed_no_send"))?;
        write(
            &path,
            &Record {
                format: "device-mcp-cipher/1".into(),
                scope,
                sealed,
            },
        )?;
        Ok(digest(&serde_json::to_vec(&payload)?))
    }
    pub fn exact(&self, key: &str, purpose: &str) -> Result<Value> {
        let scope = self.scope(key, purpose);
        let path = self.directory.join(format!(
            "{}-{}-{}.sealed.json",
            self.run, self.source, scope.record_id
        ));
        let r: Record = read(&path)?;
        ensure!(
            r.format == "device-mcp-cipher/1" && r.scope == scope && r.sealed.key_id() == self.key,
            "device_mcp_original_scope_changed"
        );
        let value: Value = serde_json::from_slice(
            &open_mcp_blob(&self.keys, &scope, &r.sealed)
                .map_err(|_| anyhow::anyhow!("device_mcp_original_cipher_invalid"))?,
        )?;
        ensure!(
            value["run"] == self.run.to_string()
                && value["source"] == self.source.to_string()
                && value["operation_key"] == key
                && value["original_binding_sha256"] == self.binding,
            "device_mcp_original_receipt_changed"
        );
        Ok(value["value"].clone())
    }
}
pub(super) fn confirm(data: &Path) -> Result<Value> {
    let d = data.join("mcp-journal");
    crate::config::private_directory(&d)?;
    let id: KeyId = read(&d.join("key-candidate.json"))?;
    let keys = NativeKeyStore::open(id.instance)
        .map_err(|_| anyhow::anyhow!("device_mcp_os_keystore_unavailable"))?;
    let proof = prove_key(&keys, &id)
        .map_err(|_| anyhow::anyhow!("device_mcp_original_key_missing_no_rekey"))?;
    let path = d.join("key-proof.json");
    if path.exists() {
        let old: KeyProof = read(&path)?;
        ensure!(old == proof, "device_mcp_original_key_differs");
    } else {
        write(&path, &proof)?;
    }
    Ok(
        json!({"format":"device-mcp-key-confirmation/1","key":id,"confirmed":true,"execution_authority":false}),
    )
}
pub(super) fn inspect(data: &Path, run: Uuid, source: Uuid) -> Result<Value> {
    let d = data.join("mcp-journal");
    ensure!(
        d.join("key-candidate.json").exists(),
        "device_mcp_original_journal_missing"
    );
    let (keys, key) = custody(&d)?;
    let mut records = Vec::new();
    let prefix = format!("{run}-{source}-");
    for entry in fs::read_dir(&d)? {
        let path = entry?.path();
        if !path.file_name().is_some_and(|n| {
            n.to_string_lossy().starts_with(&prefix)
                && n.to_string_lossy().ends_with(".sealed.json")
        }) {
            continue;
        }
        ensure!(records.len() < 4096, "device_mcp_journal_inventory_bound");
        let record: Record = read(&path)?;
        ensure!(
            record.format == "device-mcp-cipher/1"
                && record.sealed.key_id() == key
                && record.scope.owner_id == key.instance.to_string()
                && record.scope.epoch == source,
            "device_mcp_original_scope_changed"
        );
        let value: Value = serde_json::from_slice(
            &open_mcp_blob(&keys, &record.scope, &record.sealed)
                .map_err(|_| anyhow::anyhow!("device_mcp_original_cipher_invalid"))?,
        )?;
        ensure!(
            value["run"] == run.to_string() && value["source"] == source.to_string(),
            "device_mcp_original_scope_changed"
        );
        records.push(value);
    }
    Ok(
        json!({"format":"device-mcp-original-inventory/1","run":run,"source":source,"records":records,"execution_authority":false,"automatic_replay":false}),
    )
}
