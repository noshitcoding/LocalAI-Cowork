//! Private installation operator config and explicit original KMS operations.
//! No renderer, HTTP request, live-key adoption or productive cloud provisioning.
use cowork_secrets::kms::{VaultTransit, VaultTransitProfile};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{
    Store,
    server_keys::{
        ServerKeyOperationKind, ServerKeyOperationStatus, ServerKeyRegistry, ServerKeyRequest,
    },
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: u32,
    profile: VaultTransitProfile,
    token_file: PathBuf,
    ca_certificate_file: Option<PathBuf>,
    wrapping_version: u32,
    expected_registry_sha256: String,
    expected_operator_public_key_sha256: String,
}
struct Configured {
    path: PathBuf,
    data: PathBuf,
    web: Option<PathBuf>,
    configuration: Configuration,
    original_sha256: Vec<u8>,
    vault: VaultTransit,
}
fn error() -> anyhow::Error {
    anyhow::anyhow!("Server key custody is invalid, unavailable or changed; no key was adopted")
}
fn configured_path() -> anyhow::Result<Option<PathBuf>> {
    match std::env::var_os("COWORK_SERVER_KMS_CONFIG_FILE") {
        Some(path) if !path.is_empty() => Ok(Some(path.into())),
        Some(_) => Err(error()),
        None => Ok(None),
    }
}
impl Configured {
    fn read(path: &Path, data: &Path, web: Option<&Path>) -> anyhow::Result<Self> {
        let raw = super::authority_token_file::read_operator_file(path, data, web, 65536)?;
        let configuration: Configuration = serde_json::from_slice(&raw).map_err(|_| error())?;
        if configuration.format != 1 || configuration.wrapping_version == 0 {
            return Err(error());
        }
        if configuration.expected_registry_sha256.len() != 64
            || !configuration
                .expected_registry_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error());
        }
        if configuration.expected_operator_public_key_sha256.len() != 64
            || !configuration
                .expected_operator_public_key_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error());
        }
        configuration.profile.validate().map_err(|_| error())?;
        let token = super::authority_token_file::read_operator_file(
            &configuration.token_file,
            data,
            web,
            8192,
        )?;
        let token_text = Zeroizing::new(
            std::str::from_utf8(&token)
                .map_err(|_| error())?
                .trim()
                .to_owned(),
        );
        let ca = configuration
            .ca_certificate_file
            .as_ref()
            .map(|p| super::authority_token_file::read_operator_file(p, data, web, 1024 * 1024))
            .transpose()?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        for part in [
            &raw[..],
            &token[..],
            ca.as_ref().map(|v| &v[..]).unwrap_or_default(),
        ] {
            context.update(&(part.len() as u64).to_le_bytes());
            context.update(part);
        }
        let original_sha256 = context.finish().as_ref().to_vec();
        let vault = VaultTransit::new(
            configuration.profile.clone(),
            token_text,
            ca.as_ref().map(|v| &v[..]),
        )
        .map_err(|_| error())?;
        Ok(Self {
            path: path.to_owned(),
            data: data.to_owned(),
            web: web.map(Path::to_owned),
            configuration,
            original_sha256,
            vault,
        })
    }
    /// Revalidate the explicit operator authority after each provider wait.
    fn current(&self) -> anyhow::Result<()> {
        let current = Self::read(&self.path, &self.data, self.web.as_deref())?;
        if current.original_sha256 != self.original_sha256 {
            return Err(error());
        }
        Ok(())
    }
}
async fn load_snapshot(
    _store: &Store,
    data: &Path,
    web: Option<&Path>,
    registry: &ServerKeyRegistry,
    configuration: Option<&Configured>,
) -> anyhow::Result<Arc<dyn KeyStore>> {
    registry.validate().map_err(|_| error())?;
    let keys: Arc<dyn KeyStore> = if let Some(profile) = &registry.profile {
        let configuration = configuration.ok_or_else(error)?;
        if &configuration.configuration.profile != profile {
            return Err(error());
        }
        if configuration.configuration.expected_registry_sha256 != registry.sha256()? {
            return Err(error());
        }
        let keys = configuration
            .vault
            .confirm_keys(&registry.envelopes()?)
            .await
            .map_err(|_| error())?;
        configuration.current()?;
        Arc::new(
            keys.bind_stable_purposes(registry.stable.clone())
                .map_err(|_| error())?,
        )
    } else {
        let (keys, id) = super::team_key_file::open_existing(data, web)?;
        if id != registry.active {
            return Err(error());
        }
        keys
    };
    for key in &registry.keys {
        cowork_secrets::verify_key(keys.as_ref(), &key.proof).map_err(|_| error())?;
    }
    // This snapshot is returned only after the caller rechecks the SQL epoch.
    let origin =
        cowork_identity::audit::AuditAuthority::new_operator(keys.clone(), registry.active)
            .map_err(|_| error())?
            .public_key_sha256()
            .map_err(|_| error())?;
    if configuration.is_some_and(|configured| {
        configured.configuration.expected_operator_public_key_sha256 != origin
    }) {
        return Err(error());
    }
    Ok(keys)
}
pub(super) async fn open(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
) -> anyhow::Result<(Arc<dyn KeyStore>, KeyId)> {
    if let Some(registry) = store.server_key_registry().await? {
        if registry.pending.is_some() {
            anyhow::bail!(
                "A retained server-key operation requires explicit operator reconciliation before workers can start"
            );
        }
        let configuration = configured_path()?
            .map(|p| Configured::read(&p, data, web))
            .transpose()?;
        if registry.profile.is_none() && configuration.is_some() {
            anyhow::bail!("KMS activation requires an explicit original import-kms operation");
        }
        let keys = load_snapshot(store, data, web, &registry, configuration.as_ref()).await?;
        store.confirm_server_key_snapshot(&registry).await?;
        store
            .confirm_server_audit_snapshot(keys.clone(), registry.active)
            .await?;
        return Ok((keys, registry.active));
    }
    if configured_path()?.is_some() {
        anyhow::bail!("KMS activation requires explicit operator initialization and import-kms");
    }
    let (keys, id) = if store.server_key_has_existing_ciphertext().await? {
        super::team_key_file::open_existing(data, web)?
    } else {
        super::team_key_file::open(data, web)?
    };
    let registry = store
        .initialize_server_key_registry(keys.as_ref(), id)
        .await?;
    store.confirm_server_key_snapshot(&registry).await?;
    Ok((keys, id))
}
struct Candidate {
    id: KeyId,
    material: Zeroizing<[u8; 32]>,
}
impl KeyStore for Candidate {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if id != &self.id {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(Zeroizing::new(*self.material))
    }
}
fn uuid(value: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| error())?;
    if id.is_nil() || id.to_string() != value {
        return Err(error());
    }
    Ok(id)
}
fn flag(args: &[String], name: &str) -> anyhow::Result<String> {
    let values = args
        .windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(error());
    }
    Ok(values[0].clone())
}
fn parse_flags(args: &[String], allowed: &[&str]) -> anyhow::Result<()> {
    if !args.len().is_multiple_of(2) {
        return Err(error());
    }
    let mut seen = std::collections::BTreeSet::new();
    for pair in args.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].starts_with("--")
            || !seen.insert(pair[0].as_str())
        {
            return Err(error());
        }
    }
    Ok(())
}
fn print(value: &impl serde::Serialize) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|_| error())?
    );
    Ok(())
}
/// Called before API, external stores, devices or workers are constructed.
/// Commands require an explicit nonsecret installation UUID and original op ID.
pub(super) async fn command(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
    args: &[String],
) -> anyhow::Result<()> {
    let kind = args.first().map(String::as_str).ok_or_else(error)?;
    if kind == "initialize" {
        if args.len() != 1 {
            return Err(error());
        }
        let registered = store.server_key_registry().await?;
        if let Some(existing) = &registered
            && (existing.profile.is_some() || existing.pending.is_some())
        {
            return Err(error());
        }
        let (keys, id) =
            if registered.is_some() || store.server_key_has_existing_ciphertext().await? {
                super::team_key_file::open_existing(data, web)?
            } else {
                super::team_key_file::open(data, web)?
            };
        let registry = store
            .initialize_server_key_registry(keys.as_ref(), id)
            .await?;
        let origin = cowork_identity::audit::AuditAuthority::new(keys, id)
            .map_err(|_| error())?
            .public_key_sha256()
            .map_err(|_| error())?;
        return print(
            &serde_json::json!({"registry":registry,"registry_sha256":registry.sha256()?,"original_audit_origin_sha256":origin}),
        );
    }
    let current = store.server_key_registry().await?.ok_or_else(error)?;
    if kind == "status" {
        if args.len() != 1 {
            return Err(error());
        }
        return print(&serde_json::json!({"registry":current,"registry_sha256":current.sha256()?}));
    }
    if kind == "receipt" {
        parse_flags(
            &args[1..],
            &["--request", "--confirm-instance", "--origin-key-sha256"],
        )?;
        if uuid(&flag(&args[1..], "--confirm-instance")?)? != current.instance_id {
            return Err(error());
        }
        let state = store
            .server_key_operation(uuid(&flag(&args[1..], "--request")?)?)
            .await?
            .ok_or_else(error)?;
        let receipt = state.receipt.ok_or_else(error)?;
        receipt.verify(&flag(&args[1..], "--origin-key-sha256")?)?;
        return print(&receipt);
    }
    if kind == "audit-plan" {
        parse_flags(
            &args[1..],
            &[
                "--request",
                "--confirm-instance",
                "--origin-key-sha256",
                "--new-origin-key-sha256",
                "--operator-key-sha256",
            ],
        )?;
        if uuid(&flag(&args[1..], "--confirm-instance")?)? != current.instance_id {
            return Err(error());
        }
        let request = uuid(&flag(&args[1..], "--request")?)?;
        let state = store
            .server_key_operation(request)
            .await?
            .ok_or_else(error)?;
        if state.operation.request.kind != ServerKeyOperationKind::RotateAuditSigner {
            return Err(error());
        }
        let plan = store
            .server_audit_rotation_plan(request)
            .await?
            .ok_or_else(error)?;
        cowork_identity::audit_origin::verify_plan(
            &plan,
            &flag(&args[1..], "--origin-key-sha256")?,
            &flag(&args[1..], "--new-origin-key-sha256")?,
        )?;
        if plan.instance_id != current.instance_id
            || plan.original_registry_sha256 != state.operation.source_sha256
        {
            return Err(error());
        }
        if let Some(receipt) = state.receipt {
            receipt.verify(&flag(&args[1..], "--operator-key-sha256")?)?;
            let sha = cowork_identity::audit::hash(&cowork_identity::audit_checkpoints::canonical(
                &plan,
            )?);
            if receipt.payload.audit_origin_plan_sha256.as_deref() != Some(sha.as_str()) {
                return Err(error());
            }
        } else {
            // Both independently pinned origin signatures bind this public plan;
            // the original operator pin is still explicit for completed replay.
            let pin = flag(&args[1..], "--operator-key-sha256")?;
            if pin.len() != 64
                || !pin
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(error());
            }
        }
        return print(&plan);
    }
    let allowed = if kind == "activate-audit" {
        &[
            "--request",
            "--confirm-instance",
            "--config",
            "--confirm-origin-key-sha256",
            "--audit-approvals-file",
        ][..]
    } else if matches!(kind, "reconcile" | "cancel") {
        &["--request", "--confirm-instance", "--config"][..]
    } else {
        &[
            "--request",
            "--confirm-instance",
            "--expected-revision",
            "--config",
        ][..]
    };
    parse_flags(&args[1..], allowed)?;
    let request = uuid(&flag(&args[1..], "--request")?)?;
    if uuid(&flag(&args[1..], "--confirm-instance")?)? != current.instance_id {
        return Err(error());
    }
    let path = PathBuf::from(flag(&args[1..], "--config")?);
    let configuration = Configured::read(&path, data, web)?;
    if kind == "activate-audit" {
        let state = store
            .server_key_operation(request)
            .await?
            .ok_or_else(error)?;
        if state.operation.request.kind != ServerKeyOperationKind::RotateAuditSigner
            || state.operation.request.profile != configuration.configuration.profile
            || state.operation.request.wrapping_version
                != configuration.configuration.wrapping_version
        {
            return Err(error());
        }
        if let Some(receipt) = state.receipt {
            receipt.verify(
                &configuration
                    .configuration
                    .expected_operator_public_key_sha256,
            )?;
            return print(&receipt);
        }
        let original = load_snapshot(
            store,
            data,
            web,
            &state.operation.source,
            Some(&configuration),
        )
        .await?;
        state.operation.verify_original_intent(original.as_ref())?;
        let next = state.operation.confirmed_registry()?;
        let keys = configuration
            .vault
            .confirm_keys(&next.envelopes()?)
            .await
            .map_err(|_| error())?;
        configuration.current()?;
        let keys: Arc<dyn KeyStore> = Arc::new(
            keys.bind_stable_purposes(next.stable.clone())
                .map_err(|_| error())?,
        );
        let raw = super::authority_token_file::read_operator_file(
            &PathBuf::from(flag(&args[1..], "--audit-approvals-file")?),
            data,
            web,
            12 * 1024 * 1024,
        )?;
        let entries: Vec<String> = serde_json::from_slice(&raw).map_err(|_| error())?;
        if entries.len() > 128 {
            return Err(error());
        }
        use base64::Engine;
        let mut approvals = Vec::new();
        for original_bytes_base64 in entries {
            if original_bytes_base64.len() > 65536 * 4 / 3 + 8 {
                return Err(error());
            }
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&original_bytes_base64)
                .map_err(|_| error())?;
            let proof: cowork_contracts::SignedAuditTrustTransition =
                serde_json::from_slice(&bytes).map_err(|_| error())?;
            approvals.push(cowork_contracts::ImportAuditTrustTransition {
                request_id: proof.proposal.request_id,
                proposal_request_id: proof.proposal.request_id,
                original_bytes_base64,
                independently_confirmed: true,
            });
        }
        configuration.current()?;
        return print(
            &store
                .commit_server_audit_rotation(
                    request,
                    keys,
                    &flag(&args[1..], "--confirm-origin-key-sha256")?,
                    approvals,
                )
                .await?,
        );
    }
    if kind == "reconcile" || kind == "cancel" {
        let state = store
            .server_key_operation(request)
            .await?
            .ok_or_else(error)?;
        if state.operation.request.profile != configuration.configuration.profile
            || state.operation.request.wrapping_version
                != configuration.configuration.wrapping_version
        {
            return Err(error());
        }
        if let Some(receipt) = state.receipt {
            receipt.verify(
                &configuration
                    .configuration
                    .expected_operator_public_key_sha256,
            )?;
            return print(&receipt);
        }
        if configuration.configuration.expected_registry_sha256 != state.operation.source_sha256 {
            return Err(error());
        }
        let keys = load_snapshot(
            store,
            data,
            web,
            &state.operation.source,
            Some(&configuration),
        )
        .await?;
        if kind == "cancel" {
            configuration.current()?;
            return print(&store.cancel_server_key_operation(request, keys).await?);
        }
        return complete(store, data, web, state, &configuration, keys).await;
    }
    let operation_kind = match kind {
        "import-kms" => ServerKeyOperationKind::ImportKms,
        "rotate" => ServerKeyOperationKind::RotateData,
        "rotate-audit" => ServerKeyOperationKind::RotateAuditSigner,
        "rewrap" => ServerKeyOperationKind::RewrapKms,
        "retire-legacy-file" => ServerKeyOperationKind::RetireLegacyFile,
        _ => return Err(error()),
    };
    let revision = flag(&args[1..], "--expected-revision")?
        .parse::<i64>()
        .map_err(|_| error())?;
    let input = ServerKeyRequest {
        format: 1,
        request_id: request,
        expected_revision: revision,
        instance_id: current.instance_id,
        kind: operation_kind,
        profile: configuration.configuration.profile.clone(),
        wrapping_version: configuration.configuration.wrapping_version,
        legacy_file: if operation_kind == ServerKeyOperationKind::RetireLegacyFile {
            if let Some(retained) = store.server_key_operation(request).await? {
                retained.operation.request.legacy_file
            } else {
                Some(super::authority_token_file::inspect_legacy_master_file(
                    data,
                    web,
                    &current.keys.first().ok_or_else(error)?.proof,
                )?)
            }
        } else {
            None
        },
    };
    if let Some(retained) = store.server_key_operation(request).await? {
        if retained.operation.request != input {
            return Err(error());
        }
        if let Some(receipt) = retained.receipt {
            receipt.verify(
                &configuration
                    .configuration
                    .expected_operator_public_key_sha256,
            )?;
            return print(&receipt);
        }
        if configuration.configuration.expected_registry_sha256 != retained.operation.source_sha256
        {
            return Err(error());
        }
        let keys = load_snapshot(
            store,
            data,
            web,
            &retained.operation.source,
            Some(&configuration),
        )
        .await?;
        return complete(store, data, web, retained, &configuration, keys).await;
    }
    let keys = load_snapshot(store, data, web, &current, Some(&configuration)).await?;
    if configuration.configuration.expected_registry_sha256 != current.sha256()? {
        return Err(error());
    }
    configuration.current()?;
    store.confirm_server_key_snapshot(&current).await?;
    let (proof, material) = match operation_kind {
        ServerKeyOperationKind::ImportKms => (
            Some(current.keys.first().ok_or_else(error)?.proof.clone()),
            None,
        ),
        ServerKeyOperationKind::RetireLegacyFile => (
            Some(current.keys.first().ok_or_else(error)?.proof.clone()),
            None,
        ),
        ServerKeyOperationKind::RewrapKms => (None, None),
        ServerKeyOperationKind::RotateData | ServerKeyOperationKind::RotateAuditSigner => {
            use ring::rand::SecureRandom;
            let mut material = Zeroizing::new([0u8; 32]);
            ring::rand::SystemRandom::new()
                .fill(material.as_mut())
                .map_err(|_| error())?;
            let candidate = Candidate {
                id: KeyId {
                    instance: current.instance_id,
                    key: Uuid::new_v4(),
                },
                material,
            };
            let proof =
                cowork_secrets::prove_key(&candidate, &candidate.id).map_err(|_| error())?;
            (Some(proof), Some(candidate.material))
        }
    };
    let state = store
        .prepare_server_key_operation(input, &current, keys.as_ref(), proof, material)
        .await?;
    complete(store, data, web, state, &configuration, keys).await
}
async fn complete(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
    mut state: ServerKeyOperationStatus,
    configuration: &Configured,
    original_keys: Arc<dyn KeyStore>,
) -> anyhow::Result<()> {
    state
        .operation
        .verify_original_intent(original_keys.as_ref())?;
    if state.operation.request.kind == ServerKeyOperationKind::RetireLegacyFile {
        configuration.current()?;
        let proof = state
            .operation
            .source
            .keys
            .first()
            .ok_or_else(error)?
            .proof
            .clone();
        return print(
            &store
                .retire_server_original_file(
                    state.operation.request.request_id,
                    original_keys,
                    |original| {
                        super::authority_token_file::retire_legacy_master_file(
                            data, web, &proof, original,
                        )
                        .map_err(|_| cowork_domain::Error::NativeKeyUnavailable)
                    },
                )
                .await?,
        );
    }
    if state.state == "prepared" {
        let mut envelopes = Vec::new();
        let operation = &state.operation;
        match operation.request.kind {
            ServerKeyOperationKind::ImportKms
            | ServerKeyOperationKind::RotateData
            | ServerKeyOperationKind::RotateAuditSigner => {
                let wrapped = configuration
                    .vault
                    .wrap(
                        operation.envelope_id.ok_or_else(error)?,
                        operation.candidate_proof.clone().ok_or_else(error)?,
                        operation.candidate_material(original_keys.as_ref())?,
                        operation.request.wrapping_version,
                    )
                    .await
                    .map_err(|_| error())?;
                configuration.current()?;
                envelopes.push(wrapped);
            }
            ServerKeyOperationKind::RewrapKms => {
                for original in operation.source.envelopes()? {
                    envelopes.push(
                        configuration
                            .vault
                            .rewrap(&original, operation.request.wrapping_version)
                            .await
                            .map_err(|_| error())?,
                    );
                    configuration.current()?;
                }
            }
            ServerKeyOperationKind::RetireLegacyFile => return Err(error()),
        }
        state = store
            .record_server_key_candidate(&state.operation, envelopes, original_keys.as_ref())
            .await?;
    }
    if let Some(receipt) = state.receipt {
        receipt.verify(
            &configuration
                .configuration
                .expected_operator_public_key_sha256,
        )?;
        return print(&receipt);
    }
    let next = state.operation.confirmed_registry()?;
    let keys = configuration
        .vault
        .confirm_keys(&next.envelopes()?)
        .await
        .map_err(|_| error())?;
    configuration.current()?;
    let keys: Arc<dyn KeyStore> = Arc::new(
        keys.bind_stable_purposes(next.stable.clone())
            .map_err(|_| error())?,
    );
    if state.operation.request.kind == ServerKeyOperationKind::RotateAuditSigner {
        let plan = store
            .stage_server_audit_rotation(state.operation.request.request_id, keys)
            .await?;
        return print(&plan);
    }
    let receipt = store
        .commit_server_key_operation(
            state.operation.request.request_id,
            keys,
            cowork_storage::server_keys::rewrap_operator_configuration,
        )
        .await?;
    print(&receipt)
}
