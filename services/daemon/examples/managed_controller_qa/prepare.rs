//! First half of the bounded native-controller QA. This uses the real Store
//! mutations; no SQL inserts, qualification minting or execution occurs here.
use anyhow::{Context, Result, ensure};
use cowork_contracts::{CreateOrder, CreateProject, ExecutionRunnerIdentity};
use cowork_domain::{
    canonical_hash,
    environments::{EnvironmentChange, EnvironmentDefinition},
    execution_targets::{target_reference, validate_runner_identity},
};
use cowork_runner_protocol::SigningKey;
use cowork_storage::Store;
use serde::Deserialize;
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    format_version: u32,
    definition: EnvironmentDefinition,
    runner: PublicRunner,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicRunner {
    runner_id: String,
    generation: u64,
    adapter_build_sha256: String,
    runner_public_key: Vec<u8>,
}

#[cfg(unix)]
fn private_root(root: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(
        root.is_absolute() && root.file_name().is_some_and(|n| n == "controller"),
        "Explicit private controller fixture root required"
    );
    ensure!(
        root.parent()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "case-storage")),
        "Bounded case-storage root required"
    );
    let owner = root
        .parent()
        .and_then(Path::parent)
        .context("Missing own fixture generation")?;
    let name = owner
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid own fixture name")?;
    ensure!(
        name.strip_prefix("cowork-runtime-volumes-").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        ),
        "Own runtime fixture generation required"
    );
    for ancestor in root.ancestors() {
        let m = std::fs::symlink_metadata(ancestor)?;
        ensure!(
            m.is_dir()
                && !m.file_type().is_symlink()
                && m.uid() == 0
                && (m.mode() & 0o022 == 0 || m.mode() & libc::S_ISVTX != 0),
            "Controller fixture ancestry changed"
        );
    }
    ensure!(
        std::fs::metadata(root)?.mode() & 0o077 == 0,
        "Controller fixture must be private"
    );
    Ok(())
}
#[cfg(not(unix))]
fn private_root(_: &Path) -> Result<()> {
    anyhow::bail!(
        "This first private architecture fixture requires Linux; Windows acceptance is separate"
    )
}

fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = root.join(name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(path)
}
fn read_definition(root: &Path) -> Result<Definition> {
    let path = root.join("controller-definition.json");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let m = file.metadata()?;
    ensure!(
        m.is_file() && m.len() > 0 && m.len() <= 2 * 1024 * 1024,
        "Bounded native definition required"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == 0 && m.nlink() == 1 && m.mode() & 0o077 == 0,
            "Private native definition required"
        );
    }
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 == m.len(), "Native definition changed");
    let value: Definition = serde_json::from_slice(&bytes)?;
    ensure!(
        value.format_version == 1,
        "Unsupported native definition handoff"
    );
    value.definition.validate()?;
    Ok(value)
}

pub async fn run(root: &Path) -> Result<()> {
    private_root(root)?;
    let input = read_definition(root)?;
    let control_bytes = SigningKey::generate_pkcs8()?;
    let control = SigningKey::from_pkcs8(&control_bytes)?;
    let control_path = write_new(root, "controller-control.pk8", &control_bytes)?;
    let identity = ExecutionRunnerIdentity {
        runner_id: input.runner.runner_id,
        generation: input.runner.generation,
        adapter_build_sha256: input.runner.adapter_build_sha256,
        control_public_key: control.public_key(),
        runner_public_key: input.runner.runner_public_key,
    };
    validate_runner_identity(&identity)?;
    let database = root.join("controller.sqlite");
    // create_new denies accidental replacement or an implicit probe against an
    // existing database. exercise opens the same committed database with mode=rw.
    write_new(root, "controller.sqlite", b"")?;
    let store = Store::connect(&format!("sqlite:{}?mode=rw", database.display())).await?;
    let project = store
        .create_project(CreateProject {
            name: "Private native controller architecture QA".into(),
        })
        .await?;
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Confirmed native copy".into(),
        })
        .await?;
    let environment = store
        .create_environment(
            &project.id,
            "Actual native fixture package",
            "native-controller-create",
        )
        .await?;
    let staged = store
        .change_environment(
            &project.id,
            &environment.id,
            environment.revision,
            "native-controller-stage",
            &EnvironmentChange::Stage {
                definition: input.definition.clone(),
            },
        )
        .await?;
    let version = staged
        .version_id
        .context("Store did not stage an environment version")?;
    let registered = store.register_execution_target(&identity, 0).await?;
    let active = store
        .set_execution_target_active(&identity, registered.revision, true)
        .await?;
    let target = target_reference(&active)?;
    let original_directory = root.join("originals");
    std::fs::create_dir(&original_directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&original_directory, std::fs::Permissions::from_mode(0o700))?;
    }
    write_new(
        &original_directory,
        "value.txt",
        b"approved HTTPS controller snapshot bytes\n",
    )?;
    let context = json!({"format_version":1,"database_url":format!("sqlite:{}?mode=rw",database.display()),
        "project_id":project.id,"order_id":order.id,"environment_id":environment.id,"version_id":version,
        "definition":input.definition,"identity":identity,"target":target,"broker_directory":root.join("broker"),
        "original_directory":original_directory,"folder_id":"source","control_key_file":control_path});
    write_new(
        root,
        "controller-context.json",
        &serde_json::to_vec(&context)?,
    )?;
    store.close().await;
    // This is the final public marker. Native activation happens only after the
    // real Store generated and durably committed its IDs and active revision.
    write_new(
        root,
        "controller-prepare.json",
        &serde_json::to_vec(&json!({"format_version":1,
        "control_public_key":control.public_key(),"control_key_file":control_path,
        "environment_id":environment.id,"version_id":version,"definition_sha256":canonical_hash(&input.definition),"target":target}))?,
    )?;
    println!(
        "controller_prepare=verified; store_ids_generated=true; target_activated=true; workload_dispatched=false"
    );
    Ok(())
}
