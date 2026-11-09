//! Local operator exchange; there is deliberately no remote orphan-delete route.
use super::*;
use cowork_executor::windows_isolation::{
    NativeObject, OrphanInspection, OrphanRetirement, OrphanSnapshot,
};
use std::{fs::File, io::Read, os::windows::fs::OpenOptionsExt, time::Instant};
use windows_sys::Win32::Storage::FileSystem::*;

const MAX: u64 = 256 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Plan {
    format: u32,
    operation_id: String,
    binding: Binding,
    inspector_build_sha256: String,
    original_ownership_sha256: String,
    original_ownership_identity: NativeObject,
    original_vhd_identity: NativeObject,
    attempt: PathBuf,
    snapshot: OrphanSnapshot,
    issued_at: i64,
    expires_at: i64,
    retirement_only: bool,
    operator_attestation_required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    format: u32,
    operation_id: String,
    original_plan_sha256: String,
    binding: Binding,
    original_owner_sid: String,
    approved_at: i64,
    original_profile_attested: bool,
    retirement_only: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    original_plan: Signed<Plan>,
    confirmation: Signed<Confirmation>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format: u32,
    original_intent: Intent,
    original_intent_sha256: String,
    retirement: OrphanRetirement,
}
struct Source {
    _handles: Vec<File>,
    _receiver_lock: File,
    configuration: Configuration,
    signer: SigningKey,
    ownership: platform::Ownership,
    ownership_sha: String,
    ownership_id: NativeObject,
    vhd_id: NativeObject,
}
fn read_held(path: &FsPath, max: u64) -> Result<(File, Vec<u8>)> {
    config::ordinary(path, false)?;
    let mut file = OpenOptions::new()
        .read(true)
        .access_mode(0x8002_0080)
        .share_mode(1)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    crate::private_secret_file::validate(&file)?;
    NativeObject::observe(&file)?;
    let length = file.metadata()?.len();
    ensure!(
        length > 0 && length <= max,
        "Original private record is oversized"
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == length,
        "Original private record changed"
    );
    Ok((file, bytes))
}
fn directory(path: &FsPath) -> Result<File> {
    config::ordinary(path, true)?;
    let file = OpenOptions::new()
        .access_mode(0x0002_0081)
        .share_mode(1 | 2)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    NativeObject::observe(&file)?;
    Ok(file)
}
fn source(config_path: &FsPath, attempt: &FsPath) -> Result<Source> {
    let (configuration_guard, _) = read_held(config_path, 65536)?;
    let (configuration, signer, _token) = config::load(config_path)?;
    let mut handles = vec![configuration_guard];
    for parent in configuration.data_directory.ancestors() {
        handles.push(directory(parent)?);
    }
    crate::private_secret_file::validate_directory(&handles[1])?;
    ensure!(
        attempt.parent() == Some(configuration.data_directory.join("attempts").as_path()),
        "Select the exact original attempt path recorded by this receiver"
    );
    let attempt_name = attempt
        .file_name()
        .and_then(|v| v.to_str())
        .context("Invalid original attempt")?;
    ensure!(
        (attempt_name.len() == 64
            && attempt_name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            || attempt_name
                .strip_prefix("qualification-")
                .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok()),
        "Unrecognized original receiver attempt"
    );
    handles.push(directory(&configuration.data_directory.join("attempts"))?);
    handles.push(directory(attempt)?);
    let receiver_path = configuration.data_directory.join("receiver.lock");
    config::ordinary(&receiver_path, false)?;
    let receiver_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(1 | 2)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(receiver_path)?;
    NativeObject::observe(&receiver_lock)?;
    crate::private_secret_file::validate(&receiver_lock)?;
    receiver_lock
        .try_lock()
        .context("Stop the original receiver before operator reconciliation")?;
    let (key_guard, _) = read_held(&configuration.runner_key_file, 16384)?;
    handles.push(key_guard);
    let (owner_guard, bytes) = read_held(&attempt.join("ownership.json"), 65536)?;
    let ownership: platform::Ownership = serde_json::from_slice(&bytes)?;
    ensure!(
        ownership.adapter == "windows_lpac_vhd"
            && ownership.owned_directory == attempt
            && uuid::Uuid::parse_str(&ownership.nonce).is_ok()
            && ownership.mount_volume.is_some(),
        "Original confirmed VHD ownership does not match this attempt"
    );
    let ownership_id = NativeObject::observe(&owner_guard)?;
    handles.push(owner_guard);
    let vhd = attempt.join(format!("{}.vhd", ownership.nonce));
    config::ordinary(&vhd, false)?;
    let vhd_guard = OpenOptions::new()
        .access_mode(0x0002_0080)
        .share_mode(1 | 2)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(vhd)?;
    let vhd_id = NativeObject::observe(&vhd_guard)?;
    handles.push(vhd_guard);
    Ok(Source {
        _handles: handles,
        _receiver_lock: receiver_lock,
        configuration,
        signer,
        ownership,
        ownership_sha: workspace::digest(&bytes),
        ownership_id,
        vhd_id,
    })
}
fn original<T: Serialize>(path: &FsPath, value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() as u64 <= MAX,
        "Original operator record is oversized"
    );
    config::write_new(path, &bytes)?;
    config::sync_directory(path.parent().context("Missing original record parent")?)?;
    Ok(bytes)
}
fn operation_directory(configuration: &Configuration, id: &str) -> Result<PathBuf> {
    ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "Invalid original operation identifier"
    );
    let root = configuration.data_directory.join("orphan-reconciliations");
    config::ordinary(&root, true)?;
    let path = root.join(id);
    config::ordinary(&path, true)?;
    Ok(path)
}
fn validate_plan(plan: &Plan, source: &Source) -> Result<()> {
    ensure!(
        plan.format == 1
            && plan.retirement_only
            && plan.operator_attestation_required
            && plan.binding == source.configuration.binding
            && plan.attempt == source.ownership.owned_directory
            && plan.original_ownership_sha256 == source.ownership_sha
            && plan.original_ownership_identity == source.ownership_id
            && plan.original_vhd_identity == source.vhd_id
            && plan.expires_at.checked_sub(plan.issued_at) == Some(15 * 60 * 1000),
        "Original operator plan no longer matches the receiver/OS ownership"
    );
    platform::validate_orphan_mount(
        &source.configuration,
        &source.ownership,
        &plan.snapshot.root,
    )?;
    Ok(())
}
fn inspect(
    config_path: &FsPath,
    attempt: &FsPath,
    capsule: &FsPath,
    output: &FsPath,
) -> Result<()> {
    config::ordinary(output.parent().context("Missing plan output parent")?, true)?;
    ensure!(
        output.is_absolute()
            && output
                .symlink_metadata()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "Plan output must be a new absolute private file"
    );
    let source = source(config_path, attempt)?;
    let inspection = platform::inspect_orphan(&source.configuration, &source.ownership, capsule)?;
    let root = source
        .configuration
        .data_directory
        .join("orphan-reconciliations");
    config::private_directory(&root)?;
    ensure!(
        std::fs::read_dir(&root)?.count() < 1024,
        "Retained operator history limit reached"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let operation = root.join(&id);
    config::private_directory(&operation)?;
    let at = platform::now();
    let plan = source.signer.sign(
        "windows_orphan_plan",
        Plan {
            format: 1,
            operation_id: id.clone(),
            binding: source.configuration.binding.clone(),
            inspector_build_sha256: workspace::digest(&std::fs::read(std::env::current_exe()?)?),
            original_ownership_sha256: source.ownership_sha,
            original_ownership_identity: source.ownership_id,
            original_vhd_identity: source.vhd_id,
            attempt: attempt.to_path_buf(),
            snapshot: inspection.snapshot.clone(),
            issued_at: at,
            expires_at: at
                .checked_add(15 * 60 * 1000)
                .context("Invalid operator clock")?,
            retirement_only: true,
            operator_attestation_required: true,
        },
    )?;
    let bytes = original(&operation.join("plan.json"), &plan)?;
    config::write_new(output, &bytes)?;
    config::sync_directory(output.parent().context("Missing plan output parent")?)?;
    println!(
        "{}",
        serde_json::json!({"operation_id":id,"plan_sha256":workspace::digest(&bytes),"plan_file":output,
        "profile_deleted":false,"confirmation_required":true,"execution_adopted":false})
    );
    Ok(())
}
type RetainedPlan = (Configuration, SigningKey, Signed<Plan>, PathBuf, Vec<File>);

fn load_retained(config_path: &FsPath, plan_file: &FsPath, expected: &str) -> Result<RetainedPlan> {
    ensure!(
        expected.len() == 64
            && expected
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Exact original plan SHA-256 required"
    );
    let (guard, bytes) = read_held(plan_file, MAX)?;
    ensure!(
        workspace::digest(&bytes) == expected,
        "Selected original plan digest differs"
    );
    let plan: Signed<Plan> = serde_json::from_slice(&bytes)?;
    let (config_guard, _) = read_held(config_path, 65536)?;
    let (configuration, signer, _token) = config::load(config_path)?;
    plan.verify(
        "windows_orphan_plan",
        &configuration.binding.runner_public_key,
    )?;
    ensure!(
        plan.payload.binding == configuration.binding
            && plan.payload.format == 1
            && plan.payload.retirement_only
            && plan.payload.operator_attestation_required,
        "Original operator binding differs"
    );
    let operation = operation_directory(&configuration, &plan.payload.operation_id)?;
    let mut guards = vec![
        guard,
        config_guard,
        directory(&operation)?,
        directory(operation.parent().context("Missing operation parent")?)?,
    ];
    let (retained_guard, retained) = read_held(&operation.join("plan.json"), MAX)?;
    ensure!(
        bytes == retained,
        "Selected plan is not the retained original plan"
    );
    guards.push(retained_guard);
    Ok((configuration, signer, plan, operation, guards))
}
fn verify_intent(intent: &Intent, plan: &Signed<Plan>, sha: &str) -> Result<()> {
    intent.confirmation.verify(
        "windows_orphan_confirmation",
        &plan.payload.binding.control_public_key,
    )?;
    let approval = &intent.confirmation.payload;
    ensure!(
        intent.original_plan == *plan
            && approval.format == 1
            && approval.operation_id == plan.payload.operation_id
            && approval.original_plan_sha256 == sha
            && approval.binding == plan.payload.binding
            && approval.original_owner_sid == plan.payload.snapshot.owner_sid
            && approval.retirement_only
            && approval.original_profile_attested
            && approval.approved_at >= plan.payload.issued_at
            && approval.approved_at < plan.payload.expires_at,
        "Retained operator confirmation differs"
    );
    Ok(())
}
fn receipt(
    configuration: &Configuration,
    plan: &Signed<Plan>,
    operation: &FsPath,
    sha: &str,
) -> Result<Option<(Signed<Receipt>, Vec<u8>)>> {
    let path = operation.join("receipt.json");
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let (_guard, bytes) = read_held(&path, MAX)?;
    let receipt: Signed<Receipt> = serde_json::from_slice(&bytes)?;
    receipt.verify(
        "windows_orphan_receipt",
        &configuration.binding.runner_public_key,
    )?;
    verify_intent(&receipt.payload.original_intent, plan, sha)?;
    let (_intent_guard, intent_bytes) = read_held(&operation.join("intent.json"), MAX)?;
    let retained_intent: Intent = serde_json::from_slice(&intent_bytes)?;
    verify_intent(&retained_intent, plan, sha)?;
    ensure!(
        receipt.payload.format == 1
            && receipt.payload.original_intent_sha256 == workspace::digest(&intent_bytes)
            && serde_json::to_vec(&retained_intent)?
                == serde_json::to_vec(&receipt.payload.original_intent)?
            && receipt.payload.retirement.original == plan.payload.snapshot
            && receipt.payload.retirement.valid_closure(),
        "Original retirement receipt is invalid"
    );
    Ok(Some((receipt, bytes)))
}
fn show_receipt(bytes: &[u8], path: &FsPath) {
    println!(
        "{}",
        serde_json::json!({"receipt_file":path,"receipt_sha256":workspace::digest(bytes),
        "original_receipt":true,"execution_adopted":false})
    );
}
fn confirm(
    config_path: &FsPath,
    plan_file: &FsPath,
    sha: &str,
    operator_key: &FsPath,
) -> Result<()> {
    let started = Instant::now();
    let (configuration, _signer, plan, operation, _guards) =
        load_retained(config_path, plan_file, sha)?;
    let (_key_guard, key_bytes) = read_held(operator_key, 16384)?;
    let controller = SigningKey::from_pkcs8(&key_bytes)?;
    ensure!(
        controller.public_key() == configuration.binding.control_public_key,
        "The original receiver control key must authorize this operator confirmation"
    );
    if let Some((_receipt, bytes)) = receipt(&configuration, &plan, &operation, sha)? {
        show_receipt(&bytes, &operation.join("receipt.json"));
        return Ok(());
    }
    let source = source(config_path, &plan.payload.attempt)?;
    validate_plan(&plan.payload, &source)?;
    let intent_path = operation.join("intent.json");
    let (intent, new_intent, _intent_guard) = match intent_path.symlink_metadata() {
        Ok(_) => {
            let (guard, bytes) = read_held(&intent_path, MAX)?;
            let intent: Intent = serde_json::from_slice(&bytes)?;
            verify_intent(&intent, &plan, sha)?;
            (intent, false, Some(guard))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let at = platform::now();
            ensure!(
                at >= plan.payload.issued_at && at < plan.payload.expires_at,
                "Inspection expired; create a fresh explicit operator plan"
            );
            let intent = Intent {
                original_plan: plan.clone(),
                confirmation: controller.sign(
                    "windows_orphan_confirmation",
                    Confirmation {
                        format: 1,
                        operation_id: plan.payload.operation_id.clone(),
                        original_plan_sha256: sha.into(),
                        binding: plan.payload.binding.clone(),
                        original_owner_sid: plan.payload.snapshot.owner_sid.clone(),
                        approved_at: at,
                        original_profile_attested: true,
                        retirement_only: true,
                    },
                )?,
            };
            original(&intent_path, &intent)?;
            (intent, true, None)
        }
        Err(e) => return Err(e.into()),
    };
    let retirement = if new_intent {
        let inspection = platform::inspect_orphan(
            &source.configuration,
            &source.ownership,
            &plan.payload.snapshot.root,
        )?;
        ensure!(
            inspection.snapshot == plan.payload.snapshot
                && started.elapsed() < Duration::from_secs(30)
                && platform::now() >= plan.payload.issued_at
                && platform::now() < plan.payload.expires_at,
            "Fresh native inspection or operator deadline no longer matches"
        );
        inspection.retire(&plan.payload.snapshot)?
    } else {
        // A lost reply never grants another OS delete. Only positive absence of
        // the original profile settles the already accepted original intent.
        OrphanInspection::reconcile_absent(&plan.payload.snapshot)?
    };
    platform::validate_orphan_mount(
        &source.configuration,
        &source.ownership,
        &plan.payload.snapshot.root,
    )?;
    let (_original_guard, intent_bytes) = read_held(&intent_path, MAX)?;
    let original_intent: Intent = serde_json::from_slice(&intent_bytes)?;
    verify_intent(&original_intent, &plan, sha)?;
    ensure!(
        serde_json::to_vec(&original_intent)? == serde_json::to_vec(&intent)?,
        "Original intent changed before receipt"
    );
    let signed = source.signer.sign(
        "windows_orphan_receipt",
        Receipt {
            format: 1,
            original_intent: intent,
            original_intent_sha256: workspace::digest(&intent_bytes),
            retirement,
        },
    )?;
    let bytes = original(&operation.join("receipt.json"), &signed)?;
    show_receipt(&bytes, &operation.join("receipt.json"));
    Ok(())
}
pub fn command(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("inspect") if args.len() == 5 => inspect(
            FsPath::new(&args[1]),
            FsPath::new(&args[2]),
            FsPath::new(&args[3]),
            FsPath::new(&args[4]),
        ),
        Some("confirm") if args.len() == 5 => confirm(
            FsPath::new(&args[1]),
            FsPath::new(&args[2]),
            &args[3],
            FsPath::new(&args[4]),
        ),
        Some("receipt") if args.len() == 4 => {
            let (configuration, _signer, plan, operation, _guards) =
                load_retained(FsPath::new(&args[1]), FsPath::new(&args[2]), &args[3])?;
            let (_receipt, bytes) = receipt(&configuration, &plan, &operation, &args[3])?
                .context("Original operation remains unresolved")?;
            show_receipt(&bytes, &operation.join("receipt.json"));
            Ok(())
        }
        _ => anyhow::bail!(
            "Usage: orphan inspect ABS_CONFIG ABS_ATTEMPT ABS_CAPSULE NEW_ABS_PLAN; orphan confirm ABS_CONFIG ABS_PLAN EXACT_SHA256 ABS_CONTROL_KEY; orphan receipt ABS_CONFIG ABS_PLAN EXACT_SHA256"
        ),
    }
}

/// Startup cannot detach the original VHD while its operator reply is unresolved.
/// A confirmed original receipt survives later detach and retained-work cleanup.
pub fn confirmed(configuration: &Configuration, root: &FsPath) -> Result<()> {
    let retirement = cowork_executor::windows_isolation::retired_marker(root)?;
    ensure!(
        retirement.original.root == root.canonicalize()?,
        "Foreign operator closure"
    );
    let history = configuration.data_directory.join("orphan-reconciliations");
    config::ordinary(&history, true)?;
    let mut entries = std::fs::read_dir(&history)?;
    let mut found = false;
    for _ in 0..1024 {
        let Some(entry) = entries.next() else {
            ensure!(
                found,
                "Original operator receipt must be reconciled before disk detach"
            );
            return Ok(());
        };
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_none_or(|v| uuid::Uuid::parse_str(v).is_err())
        {
            continue;
        }
        let path = entry.path();
        let plan_path = path.join("plan.json");
        match plan_path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        let bytes = config::private_read(&plan_path, MAX)?;
        let plan: Signed<Plan> = serde_json::from_slice(&bytes)?;
        if plan.payload.snapshot != retirement.original {
            continue;
        }
        plan.verify(
            "windows_orphan_plan",
            &configuration.binding.runner_public_key,
        )?;
        ensure!(
            plan.payload.binding == configuration.binding
                && plan.payload.format == 1
                && plan.payload.retirement_only
                && plan.payload.operator_attestation_required,
            "Original operator binding differs"
        );
        let Some((signed, _)) = receipt(configuration, &plan, &path, &workspace::digest(&bytes))?
        else {
            continue;
        };
        ensure!(
            signed.payload.retirement == retirement,
            "Original operator closure conflicts"
        );
        found = true;
    }
    ensure!(
        entries.next().is_none() && found,
        "Original operator history exceeds the inspection limit or receipt is missing"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_private_receiver_directory_preserves_native_identity_and_original_authority_bytes()
    {
        struct Fixture {
            path: PathBuf,
            id: NativeObject,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let Ok(file) = directory(&self.path) else {
                    return;
                };
                if NativeObject::observe(&file).ok().as_ref() != Some(&self.id) {
                    return;
                }
                drop(file);
                let _ = std::fs::remove_file(self.path.join("original.json"));
                let _ = std::fs::remove_dir(&self.path);
            }
        }
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cowork-orphan-directory-{}", uuid::Uuid::new_v4()));
        config::private_directory(&path).unwrap();
        let fixture = Fixture {
            id: NativeObject::observe(&directory(&path).unwrap()).unwrap(),
            path,
        };
        let original_bytes = b"original private operator authority\n";
        config::write_new(&fixture.path.join("original.json"), original_bytes).unwrap();
        let (original_guard, before) =
            read_held(&fixture.path.join("original.json"), 1024).unwrap();
        let original_id = NativeObject::observe(&original_guard).unwrap();
        config::private_directory(&fixture.path).unwrap();
        assert_eq!(
            NativeObject::observe(&directory(&fixture.path).unwrap()).unwrap(),
            fixture.id
        );
        let (after_guard, after) = read_held(&fixture.path.join("original.json"), 1024).unwrap();
        assert_eq!(after, before);
        assert_eq!(NativeObject::observe(&after_guard).unwrap(), original_id);
        assert_eq!(after, original_bytes);
    }
}
