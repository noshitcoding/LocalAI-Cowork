//! Deliberately independent offline authority. No daemon credential, network
//! connection, default key, shared database, implicit trust, or replacement.
mod files;
use anyhow::{Context, Result, ensure};
use cowork_contracts::{
    AuditCheckpointRequest, AuditTrustTransitionProposal, AuditWitnessBinding,
    SignedAuditCheckpoint, SignedAuditTrustTransition,
};
use cowork_identity::{audit, audit_checkpoints as checkpoints, audit_trust};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: String,
    instance_id: Uuid,
    public_key_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    format: String,
    binding: AuditWitnessBinding,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    format: String,
    candidate: AuditCheckpointRequest,
    receipt: SignedAuditCheckpoint,
    permitted_new_chain: Option<Uuid>,
}
struct State {
    path: PathBuf,
    _pins: Vec<File>,
    _lock: File,
    signer: Ed25519KeyPair,
}
fn read<T: serde::de::DeserializeOwned>(path: &Path, limit: u64, private: bool) -> Result<T> {
    Ok(serde_json::from_slice(&files::read(path, limit, private)?)?)
}
impl State {
    fn open(path: &Path) -> Result<Self> {
        let pins = files::pins(path, true)?;
        let lock = files::lock(path)?;
        let seed = Zeroizing::new(files::read(&path.join("signing.seed"), 32, true)?);
        ensure!(seed.len() == 32, "Original witness seed missing");
        let signer = Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|_| anyhow::anyhow!("Witness key invalid"))?;
        let header: Header = read(&path.join("header.json"), 4096, true)?;
        ensure!(
            header.format == "cowork.audit.witness-state.v1"
                && !header.instance_id.is_nil()
                && header.public_key_sha256 == audit::hash(signer.public_key().as_ref()),
            "Original witness identity changed"
        );
        Ok(Self {
            path: path.into(),
            _pins: pins,
            _lock: lock,
            signer,
        })
    }
    fn pin(&self) -> String {
        audit::hash(self.signer.public_key().as_ref())
    }
    fn key(binding: &AuditWitnessBinding) -> String {
        format!(
            "{}-{}",
            binding.origin_id,
            audit::hash(binding.scope.as_bytes())
        )
    }
    fn policy(&self, binding: &AuditWitnessBinding) -> Result<Policy> {
        if let Some(proof) = self.incoming(binding)? {
            return Ok(Policy {
                format: "cowork.audit.witness-policy.v1".into(),
                binding: proof.proposal.next_binding,
            });
        }
        let p: Policy = read(
            &self
                .path
                .join(format!("policy-{}.json", Self::key(binding))),
            16384,
            true,
        )?;
        ensure!(
            p.format == "cowork.audit.witness-policy.v1"
                && checkpoints::canonical(&p.binding)? == checkpoints::canonical(binding)?,
            "Request does not match independently authorized origin policy"
        );
        checkpoints::verify_binding(&p.binding, &p.binding.origin_public_key_sha256, &self.pin())?;
        Ok(p)
    }
    fn transition_name(direction: &str, binding: &AuditWitnessBinding) -> String {
        format!(
            "handover-{direction}-{}-{}.json",
            Self::key(binding),
            binding.binding_id
        )
    }
    fn transition_file(
        &self,
        direction: &str,
        binding: &AuditWitnessBinding,
    ) -> Result<Option<SignedAuditTrustTransition>> {
        let path = self.path.join(Self::transition_name(direction, binding));
        if !path.try_exists()? {
            return Ok(None);
        }
        let p: SignedAuditTrustTransition = read(&path, 65536, true)?;
        let a = &p.proposal.previous_binding;
        let b = &p.proposal.next_binding;
        audit_trust::verify_transition(
            &p,
            &a.origin_public_key_sha256,
            &b.origin_public_key_sha256,
            &a.witness_public_key_sha256,
            &b.witness_public_key_sha256,
        )?;
        let expected = if direction == "out" { a } else { b };
        if checkpoints::canonical(expected)? != checkpoints::canonical(binding)? {
            anyhow::bail!("Original handover binding changed");
        }
        Ok(Some(p))
    }
    fn incoming(
        &self,
        binding: &AuditWitnessBinding,
    ) -> Result<Option<SignedAuditTrustTransition>> {
        let p = self.transition_file("in", binding)?;
        if let Some(p) = &p {
            ensure!(
                p.proposal.next_binding.witness_public_key_sha256 == self.pin(),
                "Handover recipient key changed"
            );
        }
        Ok(p)
    }
    fn inventory(&self) -> Result<(usize, u64)> {
        let mut count = 0;
        let mut total = 0u64;
        for e in std::fs::read_dir(&self.path)? {
            let e = e?;
            let m = e.metadata()?;
            ensure!(
                e.file_type()?.is_file() && !e.file_type()?.is_symlink(),
                "Unexpected witness state entry"
            );
            count += 1;
            total = total
                .checked_add(m.len())
                .context("Witness inventory overflow")?;
            ensure!(
                count <= 8192 && total <= 2 * 1024 * 1024 * 1024,
                "Witness retention limit reached"
            );
        }
        Ok((count, total))
    }
    fn operations(&self, binding: &AuditWitnessBinding) -> Result<Vec<Operation>> {
        self.inventory()?;
        let incoming = self.incoming(binding)?;
        let prefix = if incoming.is_some() {
            format!("receipt-{}-{}-", Self::key(binding), binding.binding_id)
        } else {
            format!("receipt-{}-", Self::key(binding))
        };
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.path)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .context("Invalid witness file name")?
                .to_owned();
            if name.starts_with(&prefix)
                && name.strip_prefix(&prefix).is_some_and(|suffix| {
                    suffix.len() == 11
                        && suffix.ends_with(".json")
                        && suffix.as_bytes()[..6].iter().all(|b| b.is_ascii_digit())
                })
            {
                ensure!(name.ends_with(".json"), "Invalid witness receipt name");
                files.push((name, entry.path()));
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        ensure!(files.len() <= 512, "Witness origin retention limit reached");
        let mut operations: Vec<Operation> = Vec::new();
        for (name, path) in files {
            let op: Operation = read(&path, 64 * 1024 * 1024, true)?;
            ensure!(
                op.format == "cowork.audit.witness-operation.v1"
                    && checkpoints::canonical(&op.candidate.binding)?
                        == checkpoints::canonical(binding)?,
                "Original witness operation changed"
            );
            let expected = checkpoints::witness_with_transition(
                &op.candidate,
                operations
                    .last()
                    .map(|o| &o.receipt)
                    .or_else(|| incoming.as_ref().map(|p| &p.proposal.previous_checkpoint)),
                &binding.origin_public_key_sha256,
                &self.signer,
                op.receipt.witnessed_at,
                op.permitted_new_chain,
                incoming.as_ref(),
            )?;
            ensure!(
                checkpoints::canonical(&expected)? == checkpoints::canonical(&op.receipt)?
                    && name == format!("{prefix}{:06}.json", op.receipt.witness_sequence),
                "Original witness sequence or signature changed"
            );
            operations.push(op);
        }
        Ok(operations)
    }
    fn authorize(&self, binding_path: &Path, origin_pin: &str) -> Result<AuditWitnessBinding> {
        let binding: AuditWitnessBinding = read(binding_path, 16384, false)?;
        checkpoints::verify_binding(&binding, origin_pin, &self.pin())?;
        ensure!(
            self.transition_file("out", &binding)?.is_none(),
            "Original binding has been handed over"
        );
        if self.incoming(&binding)?.is_some() {
            return Ok(binding);
        }
        ensure!(
            !self
                .path
                .join(Self::transition_name("prepared", &binding))
                .try_exists()?,
            "Original handover requires reconciliation"
        );
        let name = format!("policy-{}.json", Self::key(&binding));
        let path = self.path.join(&name);
        if path.try_exists()? {
            let policy = self.policy(&binding)?;
            return Ok(policy.binding);
        }
        let (count, total) = self.inventory()?;
        ensure!(
            count < 8192 && total < 2 * 1024 * 1024 * 1024 - 16384,
            "Witness retention limit reached"
        );
        files::publish(
            &self.path,
            &name,
            &checkpoints::canonical(&Policy {
                format: "cowork.audit.witness-policy.v1".into(),
                binding: binding.clone(),
            })?,
        )?;
        Ok(binding)
    }
    fn sign(&self, candidate_path: &Path, permit: Option<Uuid>) -> Result<SignedAuditCheckpoint> {
        let candidate: AuditCheckpointRequest = read(candidate_path, 64 * 1024 * 1024, false)?;
        self.policy(&candidate.binding)?;
        let operations = self.operations(&candidate.binding)?;
        for op in &operations {
            if op.candidate.request_id == candidate.request_id {
                ensure!(
                    checkpoints::canonical(&op.candidate)? == checkpoints::canonical(&candidate)?,
                    "Original request ID has different data"
                );
                return Ok(op.receipt.clone());
            }
        }
        ensure!(
            self.transition_file("out", &candidate.binding)?.is_none(),
            "Original binding was retired by explicit handover; only original receipt replay is permitted"
        );
        ensure!(
            operations.len() < 512,
            "Witness origin retention limit reached"
        );
        let now: i64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        if let Some(last) = operations.last() {
            ensure!(
                now >= last.receipt.witnessed_at,
                "Witness clock moved backwards"
            );
        }
        let incoming = self.incoming(&candidate.binding)?;
        let receipt = checkpoints::witness_with_transition(
            &candidate,
            operations
                .last()
                .map(|o| &o.receipt)
                .or_else(|| incoming.as_ref().map(|p| &p.proposal.previous_checkpoint)),
            &candidate.binding.origin_public_key_sha256,
            &self.signer,
            now,
            permit,
            incoming.as_ref(),
        )?;
        ensure!(
            receipt.witness_sequence <= 512,
            "Original origin/scope retention limit reached across handovers"
        );
        let name = if incoming.is_some() {
            format!(
                "receipt-{}-{}-{:06}.json",
                Self::key(&candidate.binding),
                candidate.binding.binding_id,
                receipt.witness_sequence
            )
        } else {
            format!(
                "receipt-{}-{:06}.json",
                Self::key(&candidate.binding),
                receipt.witness_sequence
            )
        };
        let op = Operation {
            format: "cowork.audit.witness-operation.v1".into(),
            candidate,
            receipt: receipt.clone(),
            permitted_new_chain: permit,
        };
        let bytes = checkpoints::canonical(&op)?;
        let (count, total) = self.inventory()?;
        ensure!(
            count < 8192
                && bytes.len() <= 64 * 1024 * 1024
                && total.saturating_add(bytes.len() as u64) <= 2 * 1024 * 1024 * 1024,
            "Witness retention limit reached"
        );
        files::publish(&self.path, &name, &bytes)?;
        Ok(receipt)
    }
    /// The recipient already has an explicitly initialized independent key.
    /// Prepared recipient -> permanent source refusal -> recipient activation.
    /// CREATE_NEW publications plus original proof replay close every ACK gap.
    fn handover(
        &self,
        recipient: &State,
        proposal_path: &Path,
        old_origin: &str,
        new_origin: &str,
        new_witness: &str,
    ) -> Result<SignedAuditTrustTransition> {
        let proposal: AuditTrustTransitionProposal = read(proposal_path, 65536, false)?;
        self.handover_proposal(recipient, proposal, old_origin, new_origin, new_witness)
    }
    fn handover_proposal(
        &self,
        recipient: &State,
        proposal: AuditTrustTransitionProposal,
        old_origin: &str,
        new_origin: &str,
        new_witness: &str,
    ) -> Result<SignedAuditTrustTransition> {
        ensure!(
            recipient.pin() == new_witness,
            "Independently confirmed recipient pin differs"
        );
        audit_trust::verify_proposal(&proposal, old_origin, new_origin, &self.pin(), new_witness)?;
        self.policy(&proposal.previous_binding)?;
        let operations = self.operations(&proposal.previous_binding)?;
        ensure!(
            operations
                .last()
                .is_some_and(|op| checkpoints::canonical(&op.receipt).ok()
                    == checkpoints::canonical(&proposal.previous_checkpoint).ok()),
            "Handover is not anchored to the original latest independent receipt"
        );
        let existing = self
            .transition_file("out", &proposal.previous_binding)?
            .or(recipient.transition_file("prepared", &proposal.next_binding)?);
        let receipt = if let Some(p) = existing {
            ensure!(
                checkpoints::canonical(&p.proposal)? == checkpoints::canonical(&proposal)?,
                "Original handover ID has different data"
            );
            p
        } else {
            ensure!(
                (!recipient
                    .path
                    .join(format!("policy-{}.json", Self::key(&proposal.next_binding)))
                    .try_exists()?
                    || (recipient.path == self.path && recipient.pin() == self.pin()))
                    && recipient.incoming(&proposal.next_binding)?.is_none(),
                "Recipient already has a policy for this origin and scope"
            );
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            audit_trust::sign_transition(
                proposal.clone(),
                old_origin,
                new_origin,
                &self.signer,
                &recipient.signer,
                now,
            )?
        };
        audit_trust::verify_transition(&receipt, old_origin, new_origin, &self.pin(), new_witness)?;
        let raw = checkpoints::canonical(&receipt)?;
        for (state, direction, binding) in [
            (recipient, "prepared", &proposal.next_binding),
            (self, "out", &proposal.previous_binding),
            (recipient, "in", &proposal.next_binding),
        ] {
            let name = Self::transition_name(direction, binding);
            let path = state.path.join(&name);
            if path.try_exists()? {
                ensure!(
                    files::read(&path, 65536, true)? == raw,
                    "Original handover publication changed"
                );
            } else {
                let (count, bytes) = state.inventory()?;
                ensure!(
                    count < 8192
                        && bytes.saturating_add(raw.len() as u64) <= 2 * 1024 * 1024 * 1024,
                    "Witness retention limit reached"
                );
                files::publish(&state.path, &name, &raw)?;
            }
        }
        Ok(receipt)
    }
}
fn init(path: &Path) -> Result<String> {
    ensure!(
        path.is_absolute() && !path.try_exists()?,
        "Initialization needs a new absolute witness state directory"
    );
    let parent = path.parent().context("Missing state parent")?;
    let _pins = files::pins(parent, false)?;
    let temporary = parent.join(format!(".witness-init-{}.tmp", Uuid::new_v4()));
    files::create_directory(&temporary)?;
    let _new_pins = files::pins(&temporary, true)?;
    let mut seed = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(seed.as_mut())
        .map_err(|_| anyhow::anyhow!("Random key generation unavailable"))?;
    let signer = Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
        .map_err(|_| anyhow::anyhow!("Witness key invalid"))?;
    let pin = audit::hash(signer.public_key().as_ref());
    files::publish(&temporary, "signing.seed", seed.as_ref())?;
    files::publish(
        &temporary,
        "header.json",
        &checkpoints::canonical(&Header {
            format: "cowork.audit.witness-state.v1".into(),
            instance_id: Uuid::new_v4(),
            public_key_sha256: pin.clone(),
        })?,
    )?;
    drop(_new_pins);
    files::no_replace(&temporary, path)?;
    files::sync_directory(parent)?;
    Ok(pin)
}
fn output(path: &Path, value: &impl Serialize) -> Result<()> {
    ensure!(
        path.is_absolute() && !path.try_exists()?,
        "Select a new absolute output file"
    );
    let parent = path.parent().context("Missing output parent")?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .context("Invalid output name")?;
    files::publish(parent, name, &checkpoints::canonical(value)?)?;
    Ok(())
}
fn arguments() -> Result<(String, BTreeMap<String, String>)> {
    let mut a = std::env::args().skip(1);
    let command = a.next().unwrap_or_else(|| "help".into());
    let mut options = BTreeMap::new();
    while let Some(k) = a.next() {
        ensure!(k.starts_with("--"), "Named options required");
        let v = a.next().context("Option value missing")?;
        ensure!(options.insert(k, v).is_none(), "Duplicate option");
    }
    Ok((command, options))
}
fn main() -> Result<()> {
    let (command, mut a) = arguments()?;
    if command == "--version" {
        println!("cowork-audit-witness {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if command == "help" {
        println!(
            "cowork-audit-witness init --state ABSOLUTE_NEW_DIRECTORY\ncowork-audit-witness status --state ABSOLUTE_DIRECTORY\ncowork-audit-witness authorize --state ABSOLUTE_DIRECTORY --binding ABSOLUTE_JSON --origin-key-sha256 INDEPENDENT_PIN\ncowork-audit-witness sign --state ABSOLUTE_DIRECTORY --request ABSOLUTE_JSON --output ABSOLUTE_NEW_JSON [--permit-new-chain EXACT_NEW_CHAIN_UUID]\ncowork-audit-witness handover --state ORIGINAL_PRIVATE_STATE --new-state EXPLICIT_RECIPIENT_STATE --proposal ABSOLUTE_JSON --origin-key-sha256 INDEPENDENT_OLD_PIN --new-origin-key-sha256 INDEPENDENT_NEW_PIN --new-witness-key-sha256 INDEPENDENT_NEW_PIN --output ABSOLUTE_NEW_JSON\ncowork-audit-witness handover-plan --state ORIGINAL_PRIVATE_STATE --new-state EXPLICIT_RECIPIENT_STATE --plan ABSOLUTE_JSON --scope EXACT_SCOPE --origin-key-sha256 INDEPENDENT_OLD_PIN --new-origin-key-sha256 INDEPENDENT_NEW_PIN --new-witness-key-sha256 INDEPENDENT_WITNESS_PIN --output ABSOLUTE_NEW_JSON\ncowork-audit-witness bundle-approvals --files-json ABSOLUTE_JSON_PATH_LIST --output ABSOLUTE_NEW_JSON\ncowork-audit-witness verify --request ABSOLUTE_JSON --receipt ABSOLUTE_JSON --origin-key-sha256 INDEPENDENT_PIN --witness-key-sha256 INDEPENDENT_PIN"
        );
        return Ok(());
    }
    if command == "verify" {
        let candidate: AuditCheckpointRequest = read(
            Path::new(&a.remove("--request").context("Request required")?),
            64 * 1024 * 1024,
            false,
        )?;
        let receipt: SignedAuditCheckpoint = read(
            Path::new(&a.remove("--receipt").context("Receipt required")?),
            16384,
            false,
        )?;
        let origin = a
            .remove("--origin-key-sha256")
            .context("Independent origin pin required")?;
        let pin = a
            .remove("--witness-key-sha256")
            .context("Independent witness pin required")?;
        ensure!(a.is_empty(), "Unknown option");
        checkpoints::verify_request(&candidate, &origin, &pin)?;
        checkpoints::verify_checkpoint_request_binding(&receipt, &candidate)?;
        println!(
            "{}",
            serde_json::json!({"valid":true,"scope":receipt.scope,"chain_id":receipt.chain_id,"sequence":receipt.head_sequence,"checkpoint_sha256":audit::hash(&checkpoints::canonical(&receipt)?)})
        );
        return Ok(());
    }
    if command == "bundle-approvals" {
        let list = PathBuf::from(
            a.remove("--files-json")
                .context("Explicit JSON file list required")?,
        );
        let out = PathBuf::from(a.remove("--output").context("New output required")?);
        ensure!(a.is_empty(), "Unknown option");
        let paths: Vec<PathBuf> = read(&list, 65536, false)?;
        ensure!(paths.len() <= 128, "Approval count limit reached");
        let mut result = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for path in paths {
            let raw = files::read(&path, 65536, false)?;
            let proof: SignedAuditTrustTransition = serde_json::from_slice(&raw)?;
            let old = &proof.proposal.previous_binding;
            let next = &proof.proposal.next_binding;
            audit_trust::verify_transition(
                &proof,
                &old.origin_public_key_sha256,
                &next.origin_public_key_sha256,
                &old.witness_public_key_sha256,
                &next.witness_public_key_sha256,
            )?;
            ensure!(
                ids.insert(proof.proposal.request_id),
                "Duplicate original approval"
            );
            result.push(checkpoints::encode_original_bytes(&raw));
        }
        output(&out, &result)?;
        println!(
            "{}",
            serde_json::json!({"original_files_bundled":result.len(),"trust_adopted":false})
        );
        return Ok(());
    }
    let path = PathBuf::from(a.remove("--state").context("State directory required")?);
    if command == "init" {
        ensure!(a.is_empty(), "Unknown option");
        println!(
            "{}",
            serde_json::json!({"public_key_sha256":init(&path)?,"initialized":true})
        );
        return Ok(());
    }
    let state = State::open(&path)?;
    match command.as_str() {
        "handover" | "handover-plan" => {
            let next = PathBuf::from(
                a.remove("--new-state")
                    .context("Explicit recipient state required")?,
            );
            let proposal = if command == "handover" {
                Some(PathBuf::from(
                    a.remove("--proposal")
                        .context("Original proposal required")?,
                ))
            } else {
                None
            };
            let plan = if command == "handover-plan" {
                Some(PathBuf::from(
                    a.remove("--plan")
                        .context("Original rotation plan required")?,
                ))
            } else {
                None
            };
            let scope = if command == "handover-plan" {
                Some(a.remove("--scope").context("Exact origin scope required")?)
            } else {
                None
            };
            let old = a
                .remove("--origin-key-sha256")
                .context("Independent original origin pin required")?;
            let new = a
                .remove("--new-origin-key-sha256")
                .context("Independent next origin pin required")?;
            let witness = a
                .remove("--new-witness-key-sha256")
                .context("Independent next witness pin required")?;
            let out = PathBuf::from(a.remove("--output").context("New output required")?);
            ensure!(a.is_empty(), "Unknown option");
            let recipient = if std::fs::canonicalize(&next)? == std::fs::canonicalize(&path)? {
                None
            } else {
                Some(State::open(&next)?)
            };
            let receipt = if let Some(proposal) = proposal {
                state.handover(
                    recipient.as_ref().unwrap_or(&state),
                    &proposal,
                    &old,
                    &new,
                    &witness,
                )?
            } else {
                let plan: cowork_contracts::AuditOriginRotationPlan = read(
                    plan.as_ref().context("Plan required")?,
                    4 * 1024 * 1024,
                    false,
                )?;
                cowork_identity::audit_origin::verify_plan(&plan, &old, &new)?;
                let proposal = plan
                    .proposals
                    .into_iter()
                    .find(|p| Some(&p.previous_binding.scope) == scope.as_ref())
                    .context("Plan does not contain the exact independently configured scope")?;
                state.handover_proposal(
                    recipient.as_ref().unwrap_or(&state),
                    proposal,
                    &old,
                    &new,
                    &witness,
                )?
            };
            output(&out, &receipt)?;
            println!(
                "{}",
                serde_json::json!({"transition_sha256":audit::hash(&checkpoints::canonical(&receipt)?),"binding_id":receipt.proposal.next_binding.binding_id,"source_signing_retired":true})
            );
        }
        "status" => {
            ensure!(a.is_empty(), "Unknown option");
            let (count, bytes) = state.inventory()?;
            println!(
                "{}",
                serde_json::json!({"public_key_sha256":state.pin(),"files":count,"bytes":bytes})
            );
        }
        "authorize" => {
            let file = PathBuf::from(a.remove("--binding").context("Binding required")?);
            let pin = a
                .remove("--origin-key-sha256")
                .context("Independent origin pin required")?;
            ensure!(a.is_empty(), "Unknown option");
            let binding = state.authorize(&file, &pin)?;
            println!(
                "{}",
                serde_json::json!({"authorized":true,"origin_id":binding.origin_id,"scope":binding.scope,"binding_id":binding.binding_id})
            );
        }
        "sign" => {
            let request = PathBuf::from(a.remove("--request").context("Request required")?);
            let out = PathBuf::from(a.remove("--output").context("New output path required")?);
            let permit = a
                .remove("--permit-new-chain")
                .map(|v| Uuid::parse_str(&v))
                .transpose()?;
            ensure!(a.is_empty(), "Unknown option");
            let receipt = state.sign(&request, permit)?;
            output(&out, &receipt)?;
            println!(
                "{}",
                serde_json::json!({"output":out,"checkpoint_sha256":audit::hash(&checkpoints::canonical(&receipt)?),"witness_sequence":receipt.witness_sequence,"operator_chain_transition":receipt.operator_chain_transition})
            );
        }
        _ => anyhow::bail!("Unknown witness command"),
    };
    Ok(())
}
