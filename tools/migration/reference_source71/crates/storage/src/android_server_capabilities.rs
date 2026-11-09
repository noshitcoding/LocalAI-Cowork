//! V1 capability names over configured V2 tools. A catalog is never run authority.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction, any::AnyRow};

const SCOPE: &str = "android_private_server_linux";

fn slot(binding: &Value) -> Result<String> {
    crate::android_private_runner::checked_binding(binding)?;
    Ok(canonical_hash(&json!({"scope":SCOPE,"host_id":binding["host_id"],"engine_id":binding["engine_id"]})))
}

fn stamp(row: &AnyRow) -> Result<Value> {
    Ok(json!({"scope":SCOPE,"slot":row.try_get::<String,_>("slot").map_err(storage_error)?,"revision":row.try_get::<i64,_>("revision").map_err(storage_error)?,"binding_sha256":row.try_get::<String,_>("binding_sha256").map_err(storage_error)?}))
}

pub(crate) async fn runner_stamp(tx: &mut Transaction<'_, Any>, binding: &Value) -> Result<Value> {
    let slot = slot(binding)?;
    let row = sqlx::query("SELECT * FROM android_private_runner_authorities WHERE slot=$1 AND active=1")
        .bind(&slot).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if row.try_get::<String,_>("host_id").map_err(storage_error)? != binding["host_id"].as_str().ok_or_else(device_invalid)?
        || row.try_get::<String,_>("engine_id").map_err(storage_error)? != binding["engine_id"].as_str().ok_or_else(device_invalid)?
        || row.try_get::<String,_>("binding_sha256").map_err(storage_error)? != canonical_hash(binding)
        || parse::<Value>(&row.try_get::<String,_>("payload").map_err(storage_error)?)? != *binding
    { return Err(Error::Forbidden); }
    stamp(&row)
}

/// The original host/engine slot is required; a different current runner cannot adopt a run.
pub(crate) async fn validate_runner(tx: &mut Transaction<'_, Any>, input: &Value) -> Result<()> {
    let binding = &input["frozen_private_runner"];
    if binding.is_null() {
        return if input["private_runner_authority"].is_null() { Ok(()) } else { Err(Error::Forbidden) };
    }
    if runner_stamp(tx, binding).await? != input["private_runner_authority"] {
        return Err(Error::Forbidden);
    }
    Ok(())
}

/// Narrow V1 vocabulary. Unsupported engines (Microsoft Office, desktop, host shell) stay absent.
pub fn capabilities_for_tools(tools: &[String], shell: bool, mcp: bool) -> Vec<String> {
    let has = |name: &str| tools.iter().any(|tool| tool == name);
    let mut names = vec!["model.chat".into(), "model.external".into()];
    if has("read_text_file") || has("execute_program") { names.push("files".into()); }
    if has("execute_program") {
        names.push("tool.program.execute".into());
        if shell { names.push("shell".into()); }
    }
    if has("browser") { names.push("browser.headless".into()); }
    if has("office") { names.extend(["office.ooxml".into(), "office.libreoffice".into()]); }
    if has("github") { names.push("tool.github".into()); }
    if mcp { names.push("tool.mcp.invoke".into()); }
    names
}

pub fn check_required(required: &[String], supported: &[String]) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    if required.len() > 64 || required.iter().any(|name| !seen.insert(name) || !supported.contains(name)) {
        return Err(Error::Validation("A required capability is unavailable for the selected tools and executor".into()));
    }
    Ok(())
}

pub fn private_tools(input: &Value) -> Result<Vec<String>> {
    let mut tools: Vec<String> = crate::android_private_tools::FILE_TOOLS.iter().map(|s| (*s).into()).collect();
    if !input["private_workflow"].is_null(){tools.push("delegate_agents".into());tools.push("read_dependency_result".into());}
    let runner = &input["frozen_private_runner"];
    if !runner.is_null() {
        crate::android_private_runner::checked_binding(runner)?;
        if runner["programs"].as_object().is_some_and(|p| !p.is_empty()) { tools.push("execute_program".into()); }
        for name in ["office", "browser"] { if runner[name] == true { tools.push(name.into()); } }
        if runner["github"] == true && !input["frozen_github_binding"].is_null() { tools.push("github".into()); }
    }
    Ok(tools)
}

pub fn private_shell(input: &Value) -> bool {
    input["frozen_private_runner"]["programs"].as_object().is_some_and(|programs| {
        programs.values().any(|path| path.as_str().is_some_and(|path| matches!(path.rsplit('/').next(), Some("bash" | "sh"))))
    })
}

fn required(spec: &Value) -> Result<Vec<String>> {
    spec["required_capabilities"].as_array().filter(|v| v.len() <= 64).ok_or_else(device_invalid)?
        .iter().map(|v| v.as_str().map(str::to_owned).ok_or_else(device_invalid)).collect()
}

/// Actor/project/provider authority is checked by the caller in this same writer transaction.
pub(crate) async fn validate_private(tx: &mut Transaction<'_, Any>, owner: Uuid, project: Uuid, spec: &Value) -> Result<()> {
    let input = &spec["input"];
    validate_runner(tx, input).await?;
    crate::android_private_workflows::execution::validate_selection(tx,owner,project,input).await?;
    crate::android_private_mcp::validate_bindings(tx, owner, project, input).await?;
    crate::android_private_github::validate_binding(tx, owner, project, input).await?;
    let selected = input.get("frozen_mcp_bindings").map(|v| v.as_array().filter(|a| a.len() <= 16).ok_or_else(device_invalid)).transpose()?;
    let mut mcp = selected.is_some_and(|bindings| !bindings.is_empty());
    if let Some(bindings) = selected {
        for binding in bindings {
            let entity = crate::android_personal::wire_uuid(binding, "entity_id")?;
            let payload: String = sqlx::query_scalar("SELECT payload FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0")
                .bind(owner.to_string()).bind(project.to_string()).bind(entity.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
            if parse::<Value>(&payload)?["transport"] == "stdio" && input["frozen_private_runner"].is_null() { return Err(Error::Forbidden); }
        }
    }
    let mut tools = private_tools(input)?;
    if let Some(selected) = input["task_config"].get("allowed_tools") {
        let selected = selected.as_array().filter(|a| a.len() <= 128).ok_or_else(device_invalid)?;
        let mut unique = std::collections::BTreeSet::new();
        for name in selected {
            let name = name.as_str().ok_or_else(device_invalid)?;
            // MCP aliases get their exact catalog check before the first model dispatch.
            let alias = name.strip_prefix("mcp_").is_some_and(|digest| digest.len() == 40 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            if !unique.insert(name) || (!tools.iter().any(|tool| tool == name) && !(mcp && alias)) { return Err(Error::Forbidden); }
        }
        tools.retain(|tool| unique.contains(tool.as_str()));
        mcp &= unique.iter().any(|name| name.starts_with("mcp_"));
    }
    let mut supported=capabilities_for_tools(&tools,private_shell(input),mcp);
    workflow_capabilities(input,&mut supported)?;
    check_required(&required(spec)?,&supported)
}

/// Recheck required capabilities against the exact initialized catalog, including task restrictions.
pub(crate) fn validate_private_spec(original: &Value, spec: &RunSpec) -> Result<()> {
    let input = &original["spec"]["input"];
    let available = private_tools(input)?;
    if spec.allowed_tools.iter().any(|tool| !available.contains(tool) && !spec.mcp_tools.iter().any(|binding| &binding.alias == tool)) {
        return Err(Error::Forbidden);
    }
    if let Some(selected) = input["task_config"].get("allowed_tools") {
        let selected = selected.as_array().ok_or_else(device_invalid)?.iter().map(|v| v.as_str().ok_or_else(device_invalid)).collect::<Result<std::collections::BTreeSet<_>>>()?;
        let actual:std::collections::BTreeSet<_>=spec.allowed_tools.iter().map(String::as_str).collect();
        if if input["private_workflow"].is_null(){selected!=actual}else{!actual.is_subset(&selected)}{return Err(Error::Forbidden)}
    }
    let mcp = spec.mcp_tools.iter().any(|binding| spec.allowed_tools.contains(&binding.alias));
    let mut supported=capabilities_for_tools(&spec.allowed_tools,private_shell(input),mcp);
    workflow_capabilities(input,&mut supported)?;
    // Graph-wide capabilities are checked against the root catalog in the same
    // authenticated authority TX. A role may intentionally hold fewer tools.
    if !input["private_workflow"].is_null(){return Ok(())}
    check_required(&required(&original["spec"])?,&supported)
}

// Only called after the active Engine stamp and original selection have been
// authenticated by validate_private in this same transaction. This pure helper
// never turns a client-supplied stamp into an admission.
fn workflow_capabilities(input:&Value,supported:&mut Vec<String>)->Result<()> {
 if input["private_workflow"].is_null(){return Ok(())}
 let frozen=&input["private_workflow"];
 if frozen["engine_stamp"]["format"]!="android-private-workflow-engine-stamp/1"||frozen["engine_stamp"]["engine"]!=frozen["engine"]{return Err(Error::Forbidden)}
 match frozen["engine"].as_str(){Some("workflow")=>supported.push("workflow.pure".into()),Some("crew")=>{
   if input["frozen_private_runner"].is_null()||frozen["engine_stamp"]["runner_binding_sha256"]!=canonical_hash(&input["frozen_private_runner"]){return Err(Error::Forbidden)}
   supported.push("crew.python".into());
 },_=>return Err(Error::Forbidden)}
 Ok(())
}

impl Store {
    /// Only operator startup calls this. A pod without a runner cannot revoke another pod's slot.
    pub async fn android_private_runner_register(&self, binding: Option<&Value>) -> Result<()> {
        let Some(binding) = binding else { return Ok(()); };
        let slot = slot(binding)?;
        let sha = canonical_hash(binding);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let old: Option<i64> = sqlx::query_scalar("SELECT revision FROM android_private_runner_authorities WHERE slot=$1")
            .bind(&slot).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        // Startup observes an existing slot. Only explicit operator activation can advance it.
        if old.is_none() {
            sqlx::query("INSERT INTO android_private_runner_authorities(slot,host_id,engine_id,revision,binding_sha256,payload,active,updated_at) VALUES($1,$2,$3,1,$4,$5,1,$6)")
                .bind(&slot).bind(binding["host_id"].as_str().ok_or_else(device_invalid)?).bind(binding["engine_id"].as_str().ok_or_else(device_invalid)?).bind(&sha).bind(json_string(binding)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }

    pub async fn android_private_runner_inspect(&self, binding: &Value) -> Result<Value> {
        let slot = slot(binding)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM android_private_runner_authorities WHERE slot=$1 AND host_id=$2 AND engine_id=$3")
            .bind(&slot).bind(binding["host_id"].as_str().ok_or_else(device_invalid)?).bind(binding["engine_id"].as_str().ok_or_else(device_invalid)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let mut value = stamp(&row)?;
        value["host_id"] = binding["host_id"].clone();
        value["engine_id"] = binding["engine_id"].clone();
        value["active"] = json!(row.try_get::<i64,_>("active").map_err(storage_error)? == 1);
        value["requested_binding_sha256"] = json!(canonical_hash(binding));
        value["binding_matches"] = json!(value["binding_sha256"] == value["requested_binding_sha256"]);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }

    pub async fn android_private_runner_activate(&self, binding: &Value, expected_revision: i64) -> Result<Value> {
        let slot = slot(binding)?;
        let next = expected_revision.checked_add(1).filter(|v| *v > 1).ok_or_else(device_invalid)?;
        let sha = canonical_hash(binding);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(row) = sqlx::query("SELECT requested_binding_sha256,receipt FROM android_private_runner_activations WHERE slot=$1 AND original_revision=$2")
            .bind(&slot).bind(expected_revision).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("requested_binding_sha256").map_err(storage_error)? != sha { return Err(Error::Conflict); }
            let receipt = parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let changed = sqlx::query("UPDATE android_private_runner_authorities SET revision=$1,binding_sha256=$2,payload=$3,active=1,updated_at=$4 WHERE slot=$5 AND host_id=$6 AND engine_id=$7 AND revision=$8")
            .bind(next).bind(&sha).bind(json_string(binding)?).bind(current).bind(&slot).bind(binding["host_id"].as_str().ok_or_else(device_invalid)?).bind(binding["engine_id"].as_str().ok_or_else(device_invalid)?).bind(expected_revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 { return Err(Error::Conflict); }
        let receipt = json!({"scope":SCOPE,"slot":slot,"host_id":binding["host_id"],"engine_id":binding["engine_id"],"original_revision":expected_revision,"binding_sha256":sha,"activated_revision":next,"activated_at":crate::android_devices::device_iso(current)?});
        sqlx::query("INSERT INTO android_private_runner_activations(slot,original_revision,requested_binding_sha256,receipt) VALUES($1,$2,$3,$4)")
            .bind(&slot).bind(expected_revision).bind(&sha).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }

    /// Explicit original-slot revocation. Re-enabling the same binding creates a new revision.
    pub async fn android_private_runner_revoke(&self, binding: &Value, expected_revision: i64) -> Result<Value> {
        let slot = slot(binding)?;
        let next = expected_revision.checked_add(1).filter(|v| *v > 1).ok_or_else(device_invalid)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let sha = canonical_hash(binding);
        if let Some(row) = sqlx::query("SELECT binding_sha256,receipt FROM android_private_runner_revocations WHERE slot=$1 AND original_revision=$2")
            .bind(&slot).bind(expected_revision).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("binding_sha256").map_err(storage_error)? != sha { return Err(Error::Conflict); }
            let receipt = parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let changed = sqlx::query("UPDATE android_private_runner_authorities SET revision=$1,active=0,updated_at=$2 WHERE slot=$3 AND host_id=$4 AND engine_id=$5 AND binding_sha256=$6 AND revision=$7 AND active=1")
            .bind(next).bind(current).bind(&slot).bind(binding["host_id"].as_str().ok_or_else(device_invalid)?).bind(binding["engine_id"].as_str().ok_or_else(device_invalid)?).bind(&sha).bind(expected_revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 { return Err(Error::Conflict); }
        let receipt = json!({"scope":SCOPE,"slot":slot,"host_id":binding["host_id"],"engine_id":binding["engine_id"],"original_revision":expected_revision,"binding_sha256":sha,"revoked_revision":next,"revoked_at":crate::android_devices::device_iso(current)?});
        sqlx::query("INSERT INTO android_private_runner_revocations(slot,original_revision,binding_sha256,receipt) VALUES($1,$2,$3,$4)")
            .bind(&slot).bind(expected_revision).bind(&sha).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }

    pub async fn android_private_runner_admission(&self, actor: &Principal, project: Uuid, binding: &Value) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute).await?;
        let stamp = runner_stamp(&mut tx, binding).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(stamp)
    }

    pub async fn android_private_runner_current(&self, actor: &Principal, binding: &Value) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity().require_global_session_in_transaction(&mut tx, actor).await.map_err(team_service::identity_error)?;
        runner_stamp(&mut tx, binding).await?;
        tx.commit().await.map_err(storage_error)
    }

    pub async fn android_private_mcp_configured(&self, actor: &Principal, runner: bool) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity().require_global_session_in_transaction(&mut tx, actor).await.map_err(team_service::identity_error)?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT b.payload FROM android_private_mcp_bindings b JOIN android_personal_projects p ON p.id=b.project_id AND p.owner_id=b.owner_id JOIN android_wire_sync_current w ON w.owner_id=b.owner_id AND w.entity_id=b.entity_id AND w.entity_type='mcp_metadata' AND w.team_id IS NULL AND w.tombstone=0 AND w.revision=b.metadata_revision AND w.digest=b.metadata_sha AND (w.project_id IS NULL OR w.project_id=b.project_id) WHERE b.owner_id=$1 AND b.deleted=0 AND p.deleted=0 LIMIT 129")
            .bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let configured = rows.iter().map(|payload| parse::<Value>(payload)).collect::<Result<Vec<_>>>()?.iter().any(|v| v["transport"] != "stdio" || runner);
        tx.commit().await.map_err(storage_error)?;
        Ok(configured)
    }

    pub async fn android_private_github_configured(&self, actor: &Principal) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity().require_global_session_in_transaction(&mut tx, actor).await.map_err(team_service::identity_error)?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_bindings b JOIN android_personal_projects p ON p.id=b.project_id AND p.owner_id=b.owner_id WHERE b.owner_id=$1 AND b.deleted=0 AND b.sealed IS NOT NULL AND p.deleted=0")
            .bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(count > 0)
    }
}
