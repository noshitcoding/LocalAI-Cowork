//! Durable ownership for actual actor-private isolated containers. A lookup never yields another create/start permit.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use crate::android_private_server::{id, leased, row};
use crate::android_private_tools as tools;
use crate::android_private_workspace as workspace;
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction};
#[path = "android_private_runner_lifecycle.rs"]
mod lifecycle;
pub use lifecycle::PrivateCreateDispatch;

pub struct PrivateContainerAdmission {
    pub id: Uuid,
    pub original_binding: Value,
    pub creation_sha256: String,
}
pub struct PrivateProgramAdmission {
    pub sequence: i64,
    pub request_sha256: String,
    pub invocation: String,
    pub program: String,
    pub arguments: Vec<String>,
    pub timeout_seconds: u64,
    pub office_action: Option<Value>,
}
fn program_action(
    tool: &ToolCall,
    runner: &Value,
) -> Result<(String, Vec<String>, u64, Option<Value>)> {
    if tool.name == "execute_program" {
        let call = cowork_domain::processes::process_invocation(tool)?;
        let program = runner["programs"]
            .get(&call.program)
            .and_then(Value::as_str)
            .ok_or(Error::Forbidden)?
            .to_owned();
        return Ok((program, call.arguments, call.timeout_seconds, None));
    }
    if tool.name != "office" || runner["office"] != true {
        return Err(Error::Forbidden);
    }
    let fields = tool.arguments.as_object().ok_or_else(device_invalid)?;
    if fields.keys().any(|k| {
        !matches!(
            k.as_str(),
            "operation" | "input" | "output" | "text" | "title"
        )
    }) {
        return Err(device_invalid());
    }
    let operation = tool.arguments["operation"]
        .as_str()
        .ok_or_else(device_invalid)?;
    let reads = matches!(
        operation,
        "read_text" | "read_docx" | "read_xlsx" | "read_pdf" | "read_pptx"
    );
    let writes = matches!(
        operation,
        "write_text" | "create_docx" | "create_xlsx" | "create_pptx" | "create_pdf"
    );
    if !reads && !writes && operation != "convert" {
        return Err(device_invalid());
    }
    for name in ["input", "output"] {
        if let Some(value) = tool.arguments.get(name) {
            crate::android_private_snapshots::relative(value.as_str().ok_or_else(device_invalid)?)?;
        }
    }
    if (reads || operation == "convert") && !fields.contains_key("input")
        || (writes || operation == "convert") && !fields.contains_key("output")
    {
        return Err(device_invalid());
    }
    if reads && fields.contains_key("output") {
        return Err(device_invalid());
    }
    for (name, limit) in [("text", 1024 * 1024), ("title", 200)] {
        if let Some(v) = fields.get(name) {
            let s = v.as_str().ok_or_else(device_invalid)?;
            if s.len() > limit || s.contains('\0') {
                return Err(Error::BudgetExceeded);
            }
        }
    }
    let mut action = tool.arguments.clone();
    if operation == "convert" {
        action["libreoffice"] = json!("/usr/bin/libreoffice");
    }
    Ok((
        "/opt/cowork/python/bin/python".into(),
        Vec::new(),
        120,
        Some(action),
    ))
}
fn physical(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn checked_binding(value: &Value) -> Result<()> {
    if value["format"] != 2
        || value["supervisor_uid"] != 0
        || value["process_uid"] != 1000
        || value["capabilities"] != json!(["SETUID", "SETGID", "KILL", "DAC_OVERRIDE", "CHOWN"])
        || value["runtime"] != "runsc"
        || value["network"] != "none"
        || value["host_id"]
            .as_str()
            .is_none_or(|s| Uuid::parse_str(s).is_err())
        || value["engine_id"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        || value["image"].as_str().is_none_or(|s| {
            s.rsplit_once("@sha256:").is_none_or(|(name, digest)| {
                name.is_empty() || !cowork_domain::processes::valid_sha256(digest)
            })
        })
    {
        return Err(device_invalid());
    }
    Ok(())
}
impl Store {
    pub async fn android_private_program_gate(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<crate::android_private_mcp_runtime::PrivateMcpGate> {
        use crate::android_private_mcp_runtime::PrivateMcpGate;
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let tool = tools::pending(&cp)?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let runner = &original["spec"]["input"]["frozen_private_runner"];
        checked_binding(runner)?;
        let (_, _, timeout, _) = program_action(&tool, runner)?;
        if !spec.allowed_tools.contains(&tool.name)
            || timeout > 1800
            || !spec
                .folders
                .first()
                .is_some_and(|f| f.id == tools::folder_id(run) && f.writable)
            || cp["model_steps"].as_u64().ok_or_else(device_invalid)?
                + cp["tool_steps"].as_u64().ok_or_else(device_invalid)?
                >= u64::from(spec.budgets.max_steps)
        {
            return Err(Error::Forbidden);
        }
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        if chrono::DateTime::parse_from_rfc3339(
            req["expires_at"].as_str().ok_or_else(device_invalid)?,
        )
        .map_err(|_| device_invalid())?
        .timestamp_millis()
            <= now
        {
            return Err(Error::Forbidden);
        }
        let gate = match req["state"].as_str() {
            Some("pending") => PrivateMcpGate::Waiting(req),
            Some("approved") => PrivateMcpGate::Ready,
            Some("rejected") => PrivateMcpGate::Denied(
                tools::complete_tool(
                    &mut tx,
                    &r,
                    &mut cp,
                    &tool,
                    &json!({"denied":true,"request_id":req["id"]}),
                    keys,
                    key_id,
                    now,
                )
                .await?,
            ),
            _ => return Err(Error::Conflict),
        };
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(gate)
    }
    pub async fn android_private_program_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<PrivateProgramAdmission> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let tool = tools::pending(&cp)?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let runner = &original["spec"]["input"]["frozen_private_runner"];
        checked_binding(runner)?;
        let (program, arguments, timeout_seconds, office_action) = program_action(&tool, runner)?;
        if !spec.allowed_tools.contains(&tool.name)
            || !spec.folders.first().is_some_and(|f| f.writable)
            || timeout_seconds > 1800
            || cp["deadline"].as_i64().is_none_or(|d| {
                d <= now
                    + i64::try_from(timeout_seconds)
                        .unwrap_or(i64::MAX)
                        .saturating_mul(1000)
            })
        {
            return Err(Error::BudgetExceeded);
        }
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        if req["state"] != "approved"
            || chrono::DateTime::parse_from_rfc3339(
                req["expires_at"].as_str().ok_or_else(device_invalid)?,
            )
            .map_err(|_| device_invalid())?
            .timestamp_millis()
                <= now
        {
            return Err(Error::Forbidden);
        }
        let intent = json!({"format":1,"tool":tool,"fingerprint":tools::tool_fingerprint(run,&cp,&tool),"runner":runner,"program":program,"arguments":arguments,"timeout_seconds":timeout_seconds,"office_action":office_action,"approval_id":req["id"],"approval_revision":req["revision"]});
        let sequence = tools::next_sequence(&mut tx, run).await?;
        let invocation = tools::invocation(&cp, &tool)?;
        let sealed = workspace::seal_value(
            keys,
            key_id,
            &r,
            &format!("intent:{sequence}"),
            json_string(&intent)?.as_bytes(),
        )?;
        crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        sqlx::query("INSERT INTO android_private_loop_steps(run_id,sequence,invocation_id,kind,lease_id,checkpoint_sha,request_sha,intent,state,created_at) VALUES($1,$2,$3,'process',$4,$5,$6,$7,'admitted',$8)").bind(run.to_string()).bind(sequence).bind(&invocation).bind(lease.to_string()).bind(canonical_hash(&cp)).bind(canonical_hash(&intent)).bind(json_string(&sealed)?).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        if r.try_get::<String, _>("state").map_err(storage_error)? != "running" {
            crate::android_private_server::save(&mut tx, &r, "running", None, None, now).await?;
        }
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(PrivateProgramAdmission {
            sequence,
            request_sha256: canonical_hash(&intent),
            invocation,
            program,
            arguments,
            timeout_seconds,
            office_action,
        })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_program_retain_received: Keep original run/lease/sequence/request/nonce and received program witness custody explicit"
    )]
    pub async fn android_private_program_retain_received(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        request_sha: &str,
        nonce: Uuid,
        received: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        if received["format"] != 1 || received["nonce"] != nonce.to_string() {
            return Err(Error::Forbidden);
        }
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        let step=sqlx::query("SELECT request_sha,receipt_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='process'").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if step
            .try_get::<String, _>("request_sha")
            .map_err(storage_error)?
            != request_sha
        {
            return Err(Error::Forbidden);
        }
        let digest = canonical_hash(received);
        if let Some(old) = step
            .try_get::<Option<String>, _>("receipt_sha")
            .map_err(storage_error)?
        {
            if old != digest {
                return Err(Error::Conflict);
            }
        } else {
            let bytes = json_string(received)?;
            if bytes.len() > 32 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            let sealed = workspace::seal_value(
                keys,
                key_id,
                &r,
                &format!("receipt:{sequence}"),
                bytes.as_bytes(),
            )?;
            crate::server_keys::cipher_writer(&mut tx, key_id).await?;
            sqlx::query("UPDATE android_private_loop_steps SET receipt=$1,receipt_sha=$2,state='received' WHERE run_id=$3 AND sequence=$4 AND receipt IS NULL").bind(json_string(&sealed)?).bind(digest).bind(run.to_string()).bind(sequence).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_program_received: Keep original run/lease/sequence/request/nonce and received program witness custody explicit"
    )]
    pub async fn android_private_program_received(
        &self,
        run: Uuid,
        lease: Uuid,
        sequence: i64,
        request_sha: &str,
        nonce: Uuid,
        received: &Value,
        witness: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        self.android_private_program_retain_received(
            run,
            lease,
            sequence,
            request_sha,
            nonce,
            received,
            keys,
            key_id,
        )
        .await?;
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(self, &mut tx, &r, &cp, now).await?;
        let previous = canonical_hash(&cp);
        let step=sqlx::query("SELECT intent,checkpoint_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='process'").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if step
            .try_get::<String, _>("checkpoint_sha")
            .map_err(storage_error)?
            != previous
        {
            return Err(Error::UncertainEffect);
        }
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        let tool = tools::pending(&cp)?;
        if intent["tool"] != json!(tool)
            || intent["fingerprint"] != tools::tool_fingerprint(run, &cp, &tool)
            || received["format"] != 1
            || received["nonce"] != nonce.to_string()
        {
            return Err(Error::Forbidden);
        }
        let closed=sqlx::query("SELECT original_binding,termination_witness FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='removed'").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        if parse::<Value>(
            &closed
                .try_get::<String, _>("original_binding")
                .map_err(storage_error)?,
        )? != intent["runner"]
            || parse::<Value>(
                &closed
                    .try_get::<String, _>("termination_witness")
                    .map_err(storage_error)?,
            )? != *witness
        {
            return Err(Error::Forbidden);
        }
        validate_program_result(&received["result"])?;
        let mut summary = received["result"].clone();
        if tool.name == "office" {
            use base64::Engine;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(
                    received["office_stdout"]
                        .as_str()
                        .filter(|s| s.len() <= 12 * 1024 * 1024)
                        .ok_or_else(device_invalid)?,
                )
                .map_err(|_| device_invalid())?;
            if raw.len() > 8 * 1024 * 1024
                || raw.len() as u64
                    != received["result"]["stdout"]["total_bytes"]
                        .as_u64()
                        .ok_or_else(device_invalid)?
            {
                return Err(Error::UncertainEffect);
            }
            let prefix = base64::engine::general_purpose::STANDARD
                .decode(
                    received["result"]["stdout"]["base64"]
                        .as_str()
                        .ok_or_else(device_invalid)?,
                )
                .map_err(|_| device_invalid())?;
            if raw.get(..raw.len().min(65536)) != Some(prefix.as_slice()) {
                return Err(Error::Forbidden);
            }
            if received["result"]["exit_code"] == 0 {
                let parsed: Value =
                    serde_json::from_slice(&raw).map_err(|_| Error::UncertainEffect)?;
                if parsed["operation"] != tool.arguments["operation"]
                    || parsed["output"]
                        != tool.arguments.get("output").cloned().unwrap_or(Value::Null)
                    || !parsed["text_truncated"].is_boolean()
                    || !parsed["text"].is_string()
                {
                    return Err(Error::Forbidden);
                }
                summary["response"] = parsed;
            }
        }
        promote(&mut tx, &r, &received["files"], keys, key_id, now).await?;
        cp["pending_tools"]
            .as_array_mut()
            .ok_or_else(device_invalid)?
            .remove(0);
        cp["tool_steps"] = json!(cp["tool_steps"].as_u64().ok_or_else(device_invalid)? + 1);
        cp["messages"].as_array_mut().ok_or_else(device_invalid)?.push(json!({"role":"tool","tool_call_id":tool.id,"name":tool.name,"content":json_string(&summary)?}));
        tools::set_checkpoint(&mut tx, &r, &cp, Some(&previous), keys, key_id, now).await?;
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_container_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        invocation: &str,
        binding: &Value,
    ) -> Result<PrivateContainerAdmission> {
        checked_binding(binding)?;
        valid_text(invocation, 200)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if &v["spec"]["input"]["frozen_private_runner"] != binding {
            return Err(Error::Forbidden);
        }
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_containers WHERE run_id=$1 AND invocation_id=$2",
        )
        .bind(run.to_string())
        .bind(invocation)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if pending != 0 {
            return Err(Error::UncertainEffect);
        }
        let original_invocation = invocation
            .strip_prefix("stdio:")
            .map(|entity| format!("mcp-discovery:{entity}"))
            .unwrap_or_else(|| invocation.to_owned());
        let terminal_session = invocation
            .strip_prefix("terminal:")
            .map(|s| Uuid::parse_str(s).map_err(|_| device_invalid()))
            .transpose()?;
        let original_request: String = if let Some(sid) = terminal_session {
            sqlx::query_scalar("SELECT request_sha FROM android_private_terminal_sessions WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND state='connected' AND container_nonce IS NULL AND deadline>$6")
                .bind(sid.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(now).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?
        } else {
            sqlx::query_scalar("SELECT request_sha FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2 AND lease_id=$3 AND state='admitted' AND receipt IS NULL").bind(run.to_string()).bind(original_invocation).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?
        };
        let nonce = Uuid::new_v4();
        let creation_sha256 = canonical_hash(
            &json!({"format":1,"owner":id(&r,"owner_id")?,"project":id(&r,"project_id")?,"run":run,"nonce":nonce,"invocation":invocation,"binding":binding,"original_request_sha256":original_request}),
        );
        sqlx::query("INSERT INTO android_private_containers(id,run_id,owner_id,project_id,invocation_id,lease_id,original_binding,creation_sha,state,revision,created_at,create_dispatch_admitted) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'prepared',1,$9,0)").bind(nonce.to_string()).bind(run.to_string()).bind(crate::android_private_server::id(&r,"owner_id")?.to_string()).bind(crate::android_private_server::id(&r,"project_id")?.to_string()).bind(invocation).bind(lease.to_string()).bind(json_string(binding)?).bind(&creation_sha256).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        if let Some(sid) = terminal_session
            && sqlx::query("UPDATE android_private_terminal_sessions SET container_nonce=$1,revision=revision+1 WHERE id=$2 AND container_nonce IS NULL AND state='connected'").bind(nonce.to_string()).bind(sid.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        tx.commit().await.map_err(storage_error)?;
        Ok(PrivateContainerAdmission {
            id: nonce,
            original_binding: binding.clone(),
            creation_sha256,
        })
    }
    /// Retain the exact Docker create response even when authority has since changed.
    pub async fn android_private_container_created(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        stdout: &str,
        container: &str,
    ) -> Result<()> {
        if !physical(container) || stdout.trim() != container || stdout.len() > 256 {
            return Err(Error::UncertainEffect);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let changed=sqlx::query("UPDATE android_private_containers SET container_id=$1,creation_receipt=$2,state='created',revision=revision+1 WHERE id=$3 AND run_id=$4 AND lease_id=$5 AND state='prepared' AND container_id IS NULL AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(container).bind(json_string(&json!({"format":1,"stdout":stdout,"container_id":container}))?).bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// The original launcher acknowledgement is observation, never another launch permit.
    pub async fn android_private_container_started(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        container: &str,
        binding: &Value,
        original_ack: &[u8],
    ) -> Result<()> {
        if original_ack.len() > 1024 || !physical(container) {
            return Err(Error::Forbidden);
        }
        let ack: Value = serde_json::from_slice(original_ack).map_err(|_| device_invalid())?;
        if ack != json!({"format":1,"ready":true,"nonce":nonce}) {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND container_id=$4 AND original_binding=$5 AND creation_receipt IS NOT NULL AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL AND state IN('start_admitted','running','terminated','removed','uncertain')").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(container).bind(json_string(binding)?).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        use base64::Engine;
        let receipt = json!({"nonce":nonce,"container_id":container,"original_ack_base64":base64::engine::general_purpose::STANDARD.encode(original_ack),"original_ack_sha256":hash(original_ack)});
        let previous:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_private_server_events WHERE run_id=$1 ORDER BY sequence DESC LIMIT 1001").bind(run.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for text in previous {
            let prior: Value = parse(&text)?;
            if prior["event_type"] == "private_container_started"
                && prior["payload"]["nonce"] == nonce.to_string()
            {
                if prior["payload"] != receipt {
                    return Err(Error::Conflict);
                }
                tx.commit().await.map_err(storage_error)?;
                return Ok(());
            }
        }
        sqlx::query("UPDATE android_private_containers SET state='running',revision=revision+1 WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND container_id=$4 AND state='start_admitted' AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(container).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_private_server::event(
            &mut tx,
            id(&r, "id")?,
            "private_container_started",
            &receipt,
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_container_start_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        container: &str,
        binding: &Value,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        if !physical(container) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let registered: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        crate::android_server_capabilities::validate_runner(&mut tx, &registered["spec"]["input"])
            .await?;
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if n > 0 {
            let cp = tools::checkpoint(&mut tx, &r, keys).await?;
            if !crate::android_private_terminal::container_authority(self, &mut tx, &r, nonce, keys)
                .await?
            {
                tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
            }
        }
        let changed=sqlx::query("UPDATE android_private_containers SET state='start_admitted',revision=revision+1 WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND container_id=$4 AND original_binding=$5 AND state='created' AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(container).bind(json_string(binding)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::UncertainEffect);
        }
        crate::android_server_capabilities::validate_runner(&mut tx, &registered["spec"]["input"])
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_container_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        container: &str,
        binding: &Value,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let registered: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        crate::android_server_capabilities::validate_runner(&mut tx, &registered["spec"]["input"])
            .await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND container_id=$4 AND original_binding=$5 AND state='start_admitted' AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(container).bind(json_string(binding)?).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if n > 0 {
            let cp = tools::checkpoint(&mut tx, &r, keys).await?;
            if !crate::android_private_terminal::container_authority(self, &mut tx, &r, nonce, keys)
                .await?
            {
                tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
            }
        }
        crate::android_server_capabilities::validate_runner(&mut tx, &registered["spec"]["input"])
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_container_cleanup_records(&self) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        // Closed no-create entries retain their immutable receipts but do not
        // consume the bounded active ownership inventory forever.
        let rows=sqlx::query("SELECT c.*,r.state AS run_state,r.lease_id AS current_lease,r.lease_until FROM android_private_containers c JOIN android_private_server_runs r ON r.id=c.run_id WHERE c.state!='removed' AND c.original_unsent_receipt IS NULL ORDER BY c.created_at LIMIT 1001").fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let values = rows
            .iter()
            .map(|r| lifecycle::record_value(r, now))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_private_container_terminated(
        &self,
        nonce: Uuid,
        binding: &Value,
        container: &str,
        witness: &Value,
    ) -> Result<()> {
        if !physical(container)
            || witness["nonce"] != nonce.to_string()
            || witness["engine_id"] != binding["engine_id"]
            || witness["original_id"] != container
            || witness["running"] != false
            || witness["paused"] != false
            || witness["restarting"] != false
            || witness["pid"] != 0
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let original=sqlx::query("SELECT container_id,termination_witness,state FROM android_private_containers WHERE id=$1 AND original_binding=$2 AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(json_string(binding)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if original
            .try_get::<Option<String>, _>("container_id")
            .map_err(storage_error)?
            .is_some_and(|v| v != container)
        {
            return Err(Error::Forbidden);
        }
        if let Some(retained) = original
            .try_get::<Option<String>, _>("termination_witness")
            .map_err(storage_error)?
        {
            if parse::<Value>(&retained)? != *witness {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        let changed=sqlx::query("UPDATE android_private_containers SET container_id=COALESCE(container_id,$1),state='terminated',termination_witness=$2,revision=revision+1 WHERE id=$3 AND original_binding=$4 AND state!='removed' AND termination_witness IS NULL AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(container).bind(json_string(witness)?).bind(nonce.to_string()).bind(json_string(binding)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_container_create_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        binding: &Value,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.private_create_validate_in(&mut tx, run, lease, nonce, binding, keys)
            .await?;
        tx.commit().await.map_err(storage_error)
    }
    async fn private_create_validate_in(
        &self,
        tx: &mut Transaction<'_, Any>,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        binding: &Value,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let now = device_writer(tx).await?;
        crate::server_keys::maintenance_fence(tx).await?;
        let r = leased(self, tx, run, lease, now).await?;
        let input: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        crate::android_server_capabilities::validate_runner(tx, &input["spec"]["input"]).await?;
        let record=sqlx::query("SELECT invocation_id FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND original_binding=$4 AND state='prepared' AND container_id IS NULL AND create_dispatch_admitted=0 AND original_dispatch_receipt IS NULL AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(json_string(binding)?).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let invocation: String = record.try_get("invocation_id").map_err(storage_error)?;
        if invocation.starts_with("terminal:") {
            if !crate::android_private_terminal::container_authority(self, tx, &r, nonce, keys)
                .await?
            {
                return Err(Error::Forbidden);
            }
            crate::android_server_capabilities::validate_runner(tx, &input["spec"]["input"])
                .await?;
            return Ok(());
        }
        let original = invocation
            .strip_prefix("stdio:")
            .map(|s| format!("mcp-discovery:{s}"))
            .unwrap_or(invocation);
        let step=sqlx::query("SELECT sequence,intent FROM android_private_loop_steps WHERE run_id=$1 AND lease_id=$2 AND invocation_id=$3 AND state='admitted' AND receipt IS NULL").bind(run.to_string()).bind(lease.to_string()).bind(original).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let sequence: i64 = step.try_get("sequence").map_err(storage_error)?;
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("intent:{sequence}"),
            &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        if let Some(expected) = intent["thread_revision"].as_i64() {
            let current:i64=sqlx::query_scalar("SELECT revision FROM android_personal_threads WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id(&r,"thread_id")?.to_string()).bind(id(&r,"owner_id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
            if expected != current {
                return Err(Error::Forbidden);
            }
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if n > 0 {
            let cp = tools::checkpoint(tx, &r, keys).await?;
            tools::live_after_crypto(self, tx, &r, &cp).await?;
        }
        crate::android_server_capabilities::validate_runner(tx, &input["spec"]["input"]).await?;
        Ok(())
    }
    pub async fn android_private_container_live(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let registered: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        crate::android_server_capabilities::validate_runner(&mut tx, &registered["spec"]["input"])
            .await?;
        let record=sqlx::query("SELECT original_binding,state FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if record
            .try_get::<String, _>("state")
            .map_err(storage_error)?
            == "removed"
        {
            return Err(Error::Forbidden);
        }
        let value: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if value["spec"]["input"]["frozen_private_runner"]
            != parse::<Value>(
                &record
                    .try_get::<String, _>("original_binding")
                    .map_err(storage_error)?,
            )?
        {
            return Err(Error::Forbidden);
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_checkpoints WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if n > 0 {
            let cp = tools::checkpoint(&mut tx, &r, keys).await?;
            if !crate::android_private_terminal::container_authority(self, &mut tx, &r, nonce, keys)
                .await?
            {
                tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
            }
        }
        crate::android_server_capabilities::validate_runner(&mut tx, &registered["spec"]["input"])
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    /// Fresh same-engine physical witnesses may close ownership; lease expiry and missing CLI replies may not.
    pub async fn android_private_container_removed(
        &self,
        nonce: Uuid,
        binding: &Value,
        physical_id: Option<&str>,
        witness: &Value,
    ) -> Result<()> {
        if witness["engine_id"] != binding["engine_id"]
            || witness["nonce"] != nonce.to_string()
            || witness["removed"] != true
            || witness["running"] != false
            || witness["paused"] != false
            || witness["restarting"] != false
            || witness["pid"] != 0
            || witness["original_id"] != json!(physical_id)
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let observed=sqlx::query("SELECT container_id,termination_witness,state FROM android_private_containers WHERE id=$1 AND original_binding=$2 AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL AND state='terminated'").bind(nonce.to_string()).bind(json_string(binding)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let retained: Value = parse(
            &observed
                .try_get::<String, _>("termination_witness")
                .map_err(storage_error)?,
        )?;
        if witness["stop"] != retained {
            return Err(Error::Forbidden);
        }
        if let Some(original) = observed
            .try_get::<Option<String>, _>("container_id")
            .map_err(storage_error)?
            && Some(original.as_str()) != physical_id
        {
            return Err(Error::Forbidden);
        }
        let changed=sqlx::query("UPDATE android_private_containers SET state='removed',termination_witness=$1,revision=revision+1 WHERE id=$2 AND original_binding=$3 AND state!='removed' AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(json_string(witness)?).bind(nonce.to_string()).bind(json_string(binding)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_runner_files(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        let entries = workspace::entries(&mut tx, run).await?;
        let mut total = 0usize;
        let mut files = Vec::new();
        for item in entries {
            if item["kind"] == "file" {
                let bytes = workspace::version_bytes(
                    &mut tx,
                    &r,
                    item["version_id"].as_str().ok_or_else(device_invalid)?,
                    keys,
                )
                .await?;
                total = total
                    .checked_add(bytes.len())
                    .ok_or(Error::BudgetExceeded)?;
                if total > 16 * 1024 * 1024 {
                    return Err(Error::BudgetExceeded);
                }
                use base64::Engine;
                files.push(json!({"path":item["path"],"kind":"file","data":base64::engine::general_purpose::STANDARD.encode(bytes.as_slice())}));
            } else {
                files.push(json!({"path":item["path"],"kind":"directory"}));
            }
        }
        tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(files)
    }
    pub async fn android_private_runner_initial_files(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Vec<Value>> {
        use base64::Engine;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut files = Vec::new();
        let mut total = 0usize;
        let mut dirs = std::collections::BTreeSet::new();
        if let Some(snapshot) = r
            .try_get::<Option<String>, _>("snapshot_id")
            .map_err(storage_error)?
        {
            let snapshot = Uuid::parse_str(&snapshot).map_err(storage_error)?;
            let manifest = crate::android_private_snapshots::manifest(
                &mut tx,
                id(&r, "owner_id")?,
                id(&r, "project_id")?,
                snapshot,
                true,
                now,
            )
            .await?;
            if let Some(values) = manifest["directories"].as_array() {
                for path in values {
                    dirs.insert(path.as_str().ok_or_else(device_invalid)?.to_owned());
                }
            }
            for item in manifest["files"].as_array().ok_or_else(device_invalid)? {
                let path = item["path"].as_str().ok_or_else(device_invalid)?;
                let bytes = crate::android_private_snapshots::file_bytes(
                    &mut tx,
                    id(&r, "owner_id")?,
                    id(&r, "project_id")?,
                    snapshot,
                    path,
                    keys,
                    now,
                )
                .await?;
                total = total
                    .checked_add(bytes.len())
                    .ok_or(Error::BudgetExceeded)?;
                if total > 16 * 1024 * 1024 {
                    return Err(Error::BudgetExceeded);
                }
                files.push(json!({"path":path,"kind":"file","data":base64::engine::general_purpose::STANDARD.encode(bytes.as_slice())}));
            }
        }
        let attachments=sqlx::query("SELECT payload,version_id FROM android_private_loop_artifacts WHERE run_id=$1 ORDER BY id LIMIT 1001").bind(run.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if attachments.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let reserved_input = files.iter().any(|f| {
            f["path"].as_str().is_some_and(|s| {
                s.eq_ignore_ascii_case("__cowork_attachments__")
                    || s.to_lowercase().starts_with("__cowork_attachments__/")
            })
        }) || dirs.iter().any(|s| {
            s.eq_ignore_ascii_case("__cowork_attachments__")
                || s.to_lowercase().starts_with("__cowork_attachments__/")
        });
        for artifact in attachments {
            let metadata: Value = parse(
                &artifact
                    .try_get::<String, _>("payload")
                    .map_err(storage_error)?,
            )?;
            if metadata["kind"] != "attachment" {
                continue;
            }
            if reserved_input {
                return Err(Error::Conflict);
            }
            let bytes = workspace::version_bytes(
                &mut tx,
                &r,
                &artifact
                    .try_get::<String, _>("version_id")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            total = total
                .checked_add(bytes.len())
                .ok_or(Error::BudgetExceeded)?;
            if total > 16 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            files.push(json!({"path":format!("__cowork_attachments__/{}",crate::android_personal::wire_uuid(&metadata,"id")?),"kind":"file","data":base64::engine::general_purpose::STANDARD.encode(bytes.as_slice())}));
        }
        for file in &files {
            let path = file["path"].as_str().ok_or_else(device_invalid)?;
            let mut parent = path.rsplit_once('/').map(|(p, _)| p);
            while let Some(p) = parent {
                dirs.insert(p.to_owned());
                parent = p.rsplit_once('/').map(|(p, _)| p);
            }
        }
        files.extend(
            dirs.into_iter()
                .map(|path| json!({"path":path,"kind":"directory"})),
        );
        let mut names = std::collections::BTreeSet::new();
        for f in &files {
            let p = f["path"].as_str().ok_or_else(device_invalid)?;
            crate::android_private_snapshots::relative(p)?;
            if !names.insert(p.to_lowercase()) {
                return Err(Error::Conflict);
            }
        }
        if files.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        let now = device_writer(&mut tx).await?;
        leased(self, &mut tx, run, lease, now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(files)
    }
}
fn validate_program_result(v: &Value) -> Result<()> {
    use base64::Engine;
    if v["exit_code"].as_i64().is_none()
        || v["process_id"].as_u64().is_none_or(|n| n == 0)
        || v["duration_ms"].as_u64().is_none()
        || v["timed_out"].as_bool().is_none()
        || v["output_complete"] != true
    {
        return Err(Error::UncertainEffect);
    }
    for field in ["stdout", "stderr"] {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(
                v[field]["base64"]
                    .as_str()
                    .filter(|s| s.len() <= 90000)
                    .ok_or_else(device_invalid)?,
            )
            .map_err(|_| device_invalid())?;
        if raw.len() > 65536
            || v[field]["sha256"] != hash(&raw)
            || v[field]["total_bytes"]
                .as_u64()
                .is_none_or(|n| n < raw.len() as u64)
            || v[field]["truncated"].as_bool()
                != v[field]["total_bytes"]
                    .as_u64()
                    .map(|n| n > raw.len() as u64)
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
pub(crate) async fn promote(
    tx: &mut Transaction<'_, Any>,
    r: &sqlx::any::AnyRow,
    value: &Value,
    keys: &dyn KeyStore,
    key_id: KeyId,
    now: i64,
) -> Result<()> {
    use base64::Engine;
    let files = value
        .as_array()
        .filter(|v| v.len() <= 10000)
        .ok_or(Error::BudgetExceeded)?;
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0usize;
    let mut validated = Vec::new();
    for f in files {
        let path = f["path"].as_str().ok_or_else(device_invalid)?;
        crate::android_private_snapshots::relative(path)?;
        if !names.insert(path.to_lowercase()) {
            return Err(Error::Conflict);
        }
        let kind = f["kind"]
            .as_str()
            .filter(|s| matches!(*s, "file" | "directory"))
            .ok_or_else(device_invalid)?;
        let bytes = if kind == "file" {
            base64::engine::general_purpose::STANDARD
                .decode(
                    f["data"]
                        .as_str()
                        .filter(|s| s.len() <= 24 * 1024 * 1024)
                        .ok_or_else(device_invalid)?,
                )
                .map_err(|_| device_invalid())?
        } else {
            Vec::new()
        };
        total = total
            .checked_add(bytes.len())
            .ok_or(Error::BudgetExceeded)?;
        if total > 16 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        validated.push((
            path.to_owned(),
            kind.to_owned(),
            zeroize::Zeroizing::new(bytes),
        ));
    }
    for (path, _, _) in &validated {
        let mut parent = path.rsplit_once('/').map(|(p, _)| p);
        while let Some(p) = parent {
            if !validated
                .iter()
                .any(|(path, kind, _)| path == p && kind == "directory")
            {
                return Err(Error::Forbidden);
            }
            parent = p.rsplit_once('/').map(|(p, _)| p);
        }
    }
    let existing = workspace::entries(tx, id(r, "id")?).await?;
    sqlx::query("DELETE FROM android_private_loop_entries WHERE run_id=$1")
        .bind(id(r, "id")?.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    for (path, kind, bytes) in validated {
        let digest = hash(&bytes);
        let version = if let Some(previous) = existing.iter().find(|e| {
            e["path"] == path
                && e["kind"] == kind
                && e["sha256"] == digest
                && e["size"] == bytes.len()
        }) {
            previous["version_id"]
                .as_str()
                .ok_or_else(device_invalid)?
                .to_owned()
        } else {
            workspace::add_version(tx, r, &path, &kind, &bytes, keys, key_id, now).await?
        };
        workspace::put_entry(tx, id(r, "id")?, &path, &version).await?;
    }
    workspace::bounds(tx, id(r, "id")?).await
}
