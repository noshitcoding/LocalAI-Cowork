//! Full graph evidence. Expired leases and safe flags never settle effects.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_contracts::RunSpec;
use cowork_domain::workflows::{WorkflowPlan, WorkflowProgress};
fn claimed(progress: &Value, node: &str, token: &str) -> bool {
    progress["history"].as_array().is_some_and(|h| {
        h.iter().any(|v| {
            v["decision"]["event"] == "claim"
                && v["decision"]["claim"]["node_id"] == node
                && v["decision"]["claim"]["token"] == token
        })
    })
}
async fn original_context(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    sha: &str,
    keys: &dyn KeyStore,
) -> Result<Value> {
    let id:Option<String>=sqlx::query_scalar("SELECT id FROM android_private_loop_versions WHERE run_id=$1 AND digest=$2 AND kind='artifact' AND path LIKE '.cowork-system/private-workflow/v1/node-%' ORDER BY id LIMIT 1").bind(crate::android_private_server::id(r,"id")?.to_string()).bind(sha).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let cp = execution::version(tx, r, &id.ok_or(Error::UncertainEffect)?, keys).await?;
    if canonical_hash(&cp) != sha {
        return Err(Error::Forbidden);
    }
    Ok(cp)
}
pub(crate) async fn semantic(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    keys: &dyn KeyStore,
) -> Result<String> {
    let current = execution::root_checkpoint(tx, r, keys).await?;
    if canonical_hash(&current) != canonical_hash(cp) || cp["execution_kind"] != "workflow" {
        return Err(Error::Forbidden);
    }
    let plan: WorkflowPlan =
        serde_json::from_value(cp["workflow"]["plan"].clone()).map_err(storage_error)?;
    plan.validate()?;
    let progress: WorkflowProgress =
        serde_json::from_value(cp["workflow"]["progress"].clone()).map_err(storage_error)?;
    progress.validate(&plan)?;
    let history = serde_json::to_value(&progress).map_err(storage_error)?;
    let run = crate::android_private_server::id(r, "id")?;
    for query in [
        "SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND state!='received'",
        "SELECT COUNT(*) FROM android_private_loop_requests WHERE run_id=$1 AND state='pending'",
        "SELECT COUNT(*) FROM android_private_workflow_rpcs WHERE run_id=$1 AND state!='received'",
        "SELECT COUNT(*) FROM android_private_browser_requests q JOIN android_private_browser_sessions s ON s.id=q.session_id WHERE s.run_id=$1 AND q.state!='received'",
        "SELECT COUNT(*) FROM android_private_github_requests q JOIN android_private_github_sessions s ON s.id=q.session_id WHERE s.run_id=$1 AND q.state!='received'",
    ] {
        let n: i64 = sqlx::query_scalar(query)
            .bind(run.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        if n != 0 {
            return Err(Error::UncertainEffect);
        }
    }
    let nodes = sqlx::query(
        "SELECT * FROM android_private_workflow_nodes WHERE run_id=$1 ORDER BY node_id LIMIT 129",
    )
    .bind(run.to_string())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if nodes.len() > 128 {
        return Err(Error::BudgetExceeded);
    }
    let expected = cp["workflow"]["nodes"].as_array().ok_or(Error::Forbidden)?;
    if expected.len() != nodes.len() || progress.nodes().len() != nodes.len() {
        return Err(Error::UncertainEffect);
    }
    let mut ledger = Vec::new();
    let mut model_count = 0i64;
    let mut tool_count = 0i64;
    for n in &nodes {
        let node: String = n.try_get("node_id").map_err(storage_error)?;
        let state: String = n.try_get("state").map_err(storage_error)?;
        let role: String = n.try_get("role_id").map_err(storage_error)?;
        let original = expected
            .iter()
            .find(|v| v["id"] == node)
            .ok_or(Error::Forbidden)?;
        let vid = n
            .try_get::<Option<String>, _>("context_version_id")
            .map_err(storage_error)?;
        let digest = n
            .try_get::<Option<String>, _>("context_sha256")
            .map_err(storage_error)?;
        if original["state"] != state
            || original["role"] != role
            || original["revision"].as_i64()
                != Some(n.try_get::<i64, _>("revision").map_err(storage_error)?)
            || original["context_version_id"] != json!(vid)
            || original["context_sha256"] != json!(digest)
        {
            return Err(Error::UncertainEffect);
        }
        let np = &progress.nodes().get(&node).ok_or(Error::Forbidden)?.plan;
        let progress_state = &progress.nodes().get(&node).ok_or(Error::Forbidden)?.state;
        let expected_state = match progress_state {
            cowork_domain::workflows::WorkflowNodeState::WaitingTimer { .. }
            | cowork_domain::workflows::WorkflowNodeState::Skipped { .. } => {
                return Err(Error::Forbidden);
            }
            cowork_domain::workflows::WorkflowNodeState::Pending => "pending",
            cowork_domain::workflows::WorkflowNodeState::Running { .. } => "running",
            cowork_domain::workflows::WorkflowNodeState::WaitingChildren => "waiting",
            cowork_domain::workflows::WorkflowNodeState::Completed { .. } => "completed",
            cowork_domain::workflows::WorkflowNodeState::Failed { .. } => "failed",
            cowork_domain::workflows::WorkflowNodeState::Blocked => "blocked",
            cowork_domain::workflows::WorkflowNodeState::Interrupted => {
                return Err(Error::UncertainEffect);
            }
        };
        if state != expected_state {
            return Err(Error::UncertainEffect);
        }
        if np.role_id.as_deref() != Some(role.as_str()) {
            return Err(Error::Forbidden);
        }
        if let Some(vid) = vid {
            let value = execution::version(tx, r, &vid, keys).await?;
            if Some(canonical_hash(&value)) != digest
                || value["workflow_node"]["node_id"] != node
                || value["workflow_node"]["role_id"] != role
                || value["workflow_node"]["plan_sha256"] != plan.sha256
                || !value["pending_tools"].as_array().is_some_and(Vec::is_empty)
            {
                return Err(Error::UncertainEffect);
            }
            let prefix = format!("workflow:{}:", hash(node.as_bytes()));
            let mut after = 0i64;
            let mut models = 0i64;
            let mut tools = 0i64;
            let mut last = None;
            loop {
                let s=sqlx::query("SELECT * FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id LIKE $2 AND sequence>$3 ORDER BY sequence LIMIT 1").bind(run.to_string()).bind(format!("{prefix}%")).bind(after).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                let Some(s) = s else { break };
                after = s.try_get("sequence").map_err(storage_error)?;
                let receipt =
                    crate::android_private_tools::step_receipt(tx, r, after, keys).await?;
                let sha = s
                    .try_get::<Option<String>, _>("receipt_sha")
                    .map_err(storage_error)?
                    .ok_or(Error::UncertainEffect)?;
                if canonical_hash(&receipt) != sha {
                    return Err(Error::Forbidden);
                }
                let intent: Value =
                    serde_json::from_slice(&crate::android_private_workspace::open_value(
                        keys,
                        r,
                        &format!("intent:{after}"),
                        &parse(&s.try_get::<String, _>("intent").map_err(storage_error)?)?,
                    )?)
                    .map_err(storage_error)?;
                if canonical_hash(&intent)
                    != s.try_get::<String, _>("request_sha")
                        .map_err(storage_error)?
                {
                    return Err(Error::Forbidden);
                }
                let source = original_context(
                    tx,
                    r,
                    &s.try_get::<String, _>("checkpoint_sha")
                        .map_err(storage_error)?,
                    keys,
                )
                .await?;
                let header = &source["workflow_node"];
                let claim = header["claim"].as_str().ok_or(Error::Forbidden)?;
                let invocation: String = s.try_get("invocation_id").map_err(storage_error)?;
                if header["run_id"] != run.to_string()
                    || header["node_id"] != node
                    || header["role_id"] != role
                    || header["plan_sha256"] != plan.sha256
                    || header["parent_lease"]
                        != s.try_get::<String, _>("lease_id").map_err(storage_error)?
                    || !claimed(&history, &node, claim)
                    || !invocation.starts_with(&format!("{prefix}{claim}:"))
                {
                    return Err(Error::Forbidden);
                }
                let mut spec: RunSpec = serde_json::from_value(header["authorized_spec"].clone())
                    .map_err(storage_error)?;
                let actual_role = plan.roles.get(&role).ok_or(Error::Forbidden)?;
                if canonical_hash(&spec.provider) != canonical_hash(&Some(&actual_role.provider))
                    || spec.prompt != np.goal
                    || canonical_hash(&spec.budgets) != canonical_hash(&np.budgets)
                    || spec.model_binding != actual_role.model_binding
                    || canonical_hash(&spec.output) != canonical_hash(&actual_role.output)
                    || !spec
                        .allowed_tools
                        .iter()
                        .all(|t| np.rights.tools.contains(t))
                    || spec.order_id != cp["spec"]["order_id"].as_str().ok_or(Error::Forbidden)?
                    || spec.project_id
                        != cp["spec"]["project_id"].as_str().ok_or(Error::Forbidden)?
                {
                    return Err(Error::Forbidden);
                }
                let kind: String = s.try_get("kind").map_err(storage_error)?;
                if kind == "model" {
                    models += 1;
                    last = Some((after, sha.clone()));
                    if cp["workflow"]["engine"] == "crew" {
                        spec.allowed_tools.clear();
                        spec.mcp_tools.clear();
                    }
                    if receipt["wire_evidence"]["completed_stream"] != true
                        || intent["workflow_node"] != *header
                        || intent["spec_sha256"] != canonical_hash(&spec)
                    {
                        return Err(Error::UncertainEffect);
                    }
                }
                if kind == "tool"
                    || invocation[format!("{prefix}{claim}:").len()..].starts_with("tool:")
                {
                    tools += 1;
                }
                ledger.push(json!({"node":node,"sequence":after,"invocation":invocation,"request_sha256":canonical_hash(&intent),"receipt_sha256":sha,"original_lease":header["parent_lease"]}));
            }
            if value["model_steps"].as_i64() != Some(models)
                || value["tool_steps"].as_i64() != Some(tools)
                || last.is_some_and(|(sequence, sha)| {
                    value["last_model_sequence"].as_i64() != Some(sequence)
                        || value["last_receipt_sha"] != sha
                })
            {
                return Err(Error::UncertainEffect);
            }
            model_count += models;
            tool_count += tools;
            ledger.push(json!({"node":node,"context_version":vid,"context_sha256":canonical_hash(&value),"state":state,"output_sha256":n.try_get::<Option<String>,_>("output_sha256").map_err(storage_error)?}));
        } else if state != "pending" && state != "blocked" {
            return Err(Error::UncertainEffect);
        }
    }
    let mut source = String::new();
    loop {
        let rpc=sqlx::query("SELECT * FROM android_private_workflow_rpcs WHERE run_id=$1 AND source_id>$2 ORDER BY source_id LIMIT 1").bind(run.to_string()).bind(&source).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let Some(rpc) = rpc else { break };
        source = rpc.try_get("source_id").map_err(storage_error)?;
        let original = execution::version(
            tx,
            r,
            &rpc.try_get::<String, _>("request_version_id")
                .map_err(storage_error)?,
            keys,
        )
        .await?;
        let reply = execution::version(
            tx,
            r,
            &rpc.try_get::<String, _>("reply_version_id")
                .map_err(storage_error)?,
            keys,
        )
        .await?;
        let raw = STANDARD
            .decode(
                original["original_base64"]
                    .as_str()
                    .ok_or(Error::Forbidden)?,
            )
            .map_err(|_| Error::Forbidden)?;
        if hash(&raw)
            != rpc
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
            || original["source_id"] != source
            || original["request_sha256"] != hash(&raw)
            || serde_json::from_slice::<Value>(&raw).map_err(storage_error)? != original["request"]
            || original["run_id"] != run.to_string()
            || original["workflow_node"]["node_id"]
                != rpc.try_get::<String, _>("node_id").map_err(storage_error)?
            || original["lease_id"]
                != rpc
                    .try_get::<String, _>("original_lease_id")
                    .map_err(storage_error)?
            || original["container_nonce"]
                != rpc
                    .try_get::<String, _>("container_nonce")
                    .map_err(storage_error)?
            || reply["request_sha256"] != original["request_sha256"]
            || reply["source"] != source
            || reply["run_id"] != run.to_string()
            || reply["node_id"] != original["workflow_node"]["node_id"]
            || reply["container_nonce"] != original["container_nonce"]
            || Some(canonical_hash(&reply))
                != rpc
                    .try_get::<Option<String>, _>("reply_sha256")
                    .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let sequence = rpc
            .try_get::<Option<i64>, _>("step_sequence")
            .map_err(storage_error)?;
        let kind: String = rpc.try_get("kind").map_err(storage_error)?;
        if reply["ok"] == false {
            if sequence.is_some()
                || kind == "result"
                || reply["result"]
                    != json!({"never_native_admitted":true,"reason":"cancelled_before_native_admission"})
            {
                return Err(Error::Forbidden);
            }
            let initial = original_context(
                tx,
                r,
                &rpc.try_get::<String, _>("context_sha256")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            let h = &initial["workflow_node"];
            let step = if kind == "model" {
                format!(
                    "workflow:{}:{}:model:{}",
                    hash(h["node_id"].as_str().ok_or(Error::Forbidden)?.as_bytes()),
                    h["claim"].as_str().ok_or(Error::Forbidden)?,
                    initial["model_steps"]
                        .as_u64()
                        .ok_or(Error::Forbidden)?
                        .checked_add(1)
                        .ok_or(Error::BudgetExceeded)?
                )
            } else {
                format!(
                    "workflow:{}:{}:tool:{}:{}",
                    hash(h["node_id"].as_str().ok_or(Error::Forbidden)?.as_bytes()),
                    h["claim"].as_str().ok_or(Error::Forbidden)?,
                    initial["model_steps"].as_u64().ok_or(Error::Forbidden)?,
                    source
                )
            };
            let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id=$2").bind(run.to_string()).bind(step).fetch_one(&mut **tx).await.map_err(storage_error)?;
            if n != 0 {
                return Err(Error::UncertainEffect);
            }
        } else if reply["ok"] != true {
            return Err(Error::Forbidden);
        } else if kind != "result" {
            let step = sequence.ok_or(Error::UncertainEffect)?;
            let evidence = crate::android_private_tools::step_receipt(tx, r, step, keys).await?;
            if kind == "model"
                && reply["result"]["original_receipt_sha256"] != canonical_hash(&evidence)
            {
                return Err(Error::Forbidden);
            }
        } else if sequence.is_some()
            || reply["result"]["accepted_content_sha256"]
                != hash(
                    original["request"]["body"]["content"]
                        .as_str()
                        .ok_or(Error::Forbidden)?
                        .as_bytes(),
                )
        {
            return Err(Error::Forbidden);
        }
        ledger.push(json!({"crew_source":source,"request_sha256":original["request_sha256"],"reply_sha256":canonical_hash(&reply),"original_container":original["container_nonce"]}));
    }
    if cp["model_steps"].as_i64() != Some(model_count)
        || cp["tool_steps"].as_i64() != Some(tool_count)
    {
        return Err(Error::UncertainEffect);
    }
    let mut semantic = cp.clone();
    semantic["workflow"]
        .as_object_mut()
        .ok_or(Error::Forbidden)?
        .remove("terminal");
    semantic["last_receipt_sha"] = Value::Null;
    Ok(canonical_hash(
        &json!({"format":"android-private-workflow-progress-proof/1","run_id":run,"plan_sha256":plan.sha256,"checkpoint_sha256":canonical_hash(&semantic),"ledger":ledger}),
    ))
}
pub(crate) async fn verify_result(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    keys: &dyn KeyStore,
) -> Result<()> {
    let proof_sha = proof(store, tx, r, cp, keys).await?;
    let closure = &cp["workflow"]["terminal"];
    if closure["format"] != "android-private-workflow-terminal/1"
        || closure["progress_proof_sha256"] != proof_sha
        || closure["run_id"] != crate::android_private_server::id(r, "id")?.to_string()
        || closure["plan_sha256"] != cp["workflow"]["plan"]["sha256"]
        || closure["content"] != cp["last_content"]
        || canonical_hash(closure) != cp["last_receipt_sha"]
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub(crate) async fn proof(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    keys: &dyn KeyStore,
) -> Result<String> {
    let semantic = semantic(tx, r, cp, keys).await?;
    let physical = crate::android_run_recovery::physical(store, tx, r, true, keys).await?;
    Ok(canonical_hash(
        &json!({"format":"android-private-workflow-quiescence-proof/1","progress_sha256":semantic,"physical_intents_sha256":physical.0,"physical_receipts_sha256":physical.1}),
    ))
}
