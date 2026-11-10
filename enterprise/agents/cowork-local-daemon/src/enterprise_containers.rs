//! Durable original container names survive native cancellation/restart; commands are never replayed.
use super::*;
static BOOT: std::sync::OnceLock<Uuid> = std::sync::OnceLock::new();
const SCHEMA:&str="CREATE TABLE IF NOT EXISTS daemon_enterprise_containers(id TEXT PRIMARY KEY,run_id TEXT NOT NULL,encrypted_original TEXT NOT NULL,encrypted_receipt TEXT);";
#[derive(Serialize, Deserialize)]
struct Original {
    format: u16,
    boot: Uuid,
    name: String,
    run_id: Uuid,
    tool_call_id: String,
    qualification: enterprise_sandbox::Qualification,
    workspace: PathBuf,
    command: String,
    created_at: DateTime<Utc>,
}
fn boot() -> Uuid {
    *BOOT.get_or_init(Uuid::new_v4)
}
pub(super) async fn shell(
    daemon: &Daemon,
    run_id: Uuid,
    invocation: &ToolInvocation,
    q: enterprise_sandbox::Qualification,
    root: &Path,
    timeout: Duration,
) -> Result<Value> {
    let id = Uuid::new_v4();
    let name = format!("cowork-native-{}", id.simple());
    let original = Original {
        format: 1,
        boot: boot(),
        name: name.clone(),
        run_id,
        tool_call_id: invocation.id.clone(),
        qualification: q.clone(),
        workspace: root.canonicalize()?,
        command: invocation.arguments["command"]
            .as_str()
            .context("original sandbox command missing")?
            .to_owned(),
        created_at: Utc::now(),
    };
    if original.command.len() > 65536 {
        bail!("original sandbox command exceeds bound")
    };
    {
        let db = daemon.database.lock().await;
        db.execute_batch(SCHEMA)?;
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM daemon_enterprise_containers",
            [],
            |r| r.get(0),
        )?;
        if count >= 10000 {
            bail!("original container custody bound reached")
        };
        let encrypted = encrypt_secret_payload(
            &serde_json::to_vec(&original)?,
            &daemon.config.model_secret_key,
        )?;
        db.execute("INSERT INTO daemon_enterprise_containers(id,run_id,encrypted_original) VALUES(?1,?2,?3)",params![id.to_string(),run_id.to_string(),encrypted])?;
    }
    q.current().await?;
    let record: RunRecord = serde_json::from_value(get_run(daemon, run_id).await?)?;
    enterprise_local::admission(daemon, &record.spec, Some(root)).await?;
    let result = q.shell(root, &original.command, timeout, &name).await;
    let receipt = match &result {
        Ok(value) => {
            json!({"format":1,"original_container":name,"actual_output":value,"physical_stop_claimed":false})
        }
        Err(error) => {
            json!({"format":1,"original_container":name,"local_return_error":error.to_string(),"original_effects_unknown":true,"command_replayed":false,"physical_stop_claimed":false})
        }
    };
    let db = daemon.database.lock().await;
    let sealed = encrypt_secret_payload(
        &serde_json::to_vec(&receipt)?,
        &daemon.config.model_secret_key,
    )?;
    db.execute("UPDATE daemon_enterprise_containers SET encrypted_receipt=?1 WHERE id=?2 AND encrypted_receipt IS NULL",params![sealed,id.to_string()])?;
    result
}
pub(super) async fn recover(daemon: &Daemon) -> Result<Value> {
    let originals = {
        let db = daemon.database.lock().await;
        db.execute_batch(SCHEMA)?;
        let mut s=db.prepare("SELECT id,encrypted_original FROM daemon_enterprise_containers WHERE encrypted_receipt IS NULL ORDER BY id LIMIT 10000")?;
        let rows = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut facts = Vec::new();
    for (id, sealed) in originals {
        let original: Original = serde_json::from_slice(&decrypt_secret_payload(
            &sealed,
            &daemon.config.model_secret_key,
        )?)?;
        if original.boot == boot() {
            continue;
        }
        if original.format != 1
            || original.name != format!("cowork-native-{}", Uuid::parse_str(&id)?.simple())
        {
            bail!("original container custody differs")
        };
        original.qualification.current().await?;
        let mut command = tokio::process::Command::new(&original.qualification.docker);
        command.kill_on_drop(true).args([
            "--context",
            &original.qualification.context,
            "kill",
            &original.name,
        ]);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await;
        let fact = match output {
            Ok(Ok(o)) => {
                json!({"format":1,"original_container":original.name,"stop_request_exit_code":o.status.code(),"stdout":String::from_utf8_lossy(&o.stdout),"stderr":String::from_utf8_lossy(&o.stderr),"original_effects_unknown":true,"command_replayed":false,"physical_stop_claimed":false})
            }
            _ => {
                json!({"format":1,"original_container":original.name,"stop_request_return_unknown":true,"original_effects_unknown":true,"command_replayed":false,"physical_stop_claimed":false})
            }
        };
        let db = daemon.database.lock().await;
        let encrypted =
            encrypt_secret_payload(&serde_json::to_vec(&fact)?, &daemon.config.model_secret_key)?;
        db.execute("UPDATE daemon_enterprise_containers SET encrypted_receipt=?1 WHERE id=?2 AND encrypted_receipt IS NULL",params![encrypted,id])?;
        facts.push(fact)
    }
    Ok(json!(facts))
}
