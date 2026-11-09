use super::*;
use crate::mcp_tasks::McpTaskRecord;
use crate::offline_execution::OfflineToolConfiguration;
use cowork_secrets::{KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob, seal_mcp_blob};
use serde::{Deserialize, Serialize};
use sqlx::any::AnyRow;
use zeroize::Zeroizing;
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub format: u32,
    pub id: String,
    pub ticket: SignedOfflineGrant,
    pub run: Uuid,
    pub ordinal: u32,
    pub tool: OfflinePendingTool,
    pub configuration_sha256: String,
    pub endpoint: String,
    pub server_id: String,
    pub catalog_sha256: String,
    pub connection_id: String,
    pub connection_epoch: u64,
    pub original_rpc_id: String,
    pub transport_request_sha256: String,
    pub protocol: String,
    pub task_ttl_ms: Option<u64>,
    pub deadline: i64,
}
/// A source permit cannot be deserialized or cloned into another dispatch.
pub struct OfflineMcpCallPermit {
    pub(super) source: Source,
    pub(super) source_sha256: String,
}
impl OfflineMcpCallPermit {
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }
}
pub(super) fn scope(s: &Source, purpose: &str) -> McpSecretScope {
    McpSecretScope {
        owner_kind: "offline".into(),
        owner_id: s.ticket.grant.device.id.to_string(),
        project_id: s.ticket.grant.project_id.to_string(),
        server_id: s.server_id.clone(),
        record_id: s.id.clone(),
        revision: 1,
        epoch: s.ticket.grant.id,
        configuration_sha256: s.configuration_sha256.clone(),
        purpose: purpose.into(),
    }
}
/// The original signed ticket already has immutable local custody. Bind its
/// entire digest instead of duplicating a permitted 2 MiB snapshot in each blob.
pub(super) fn sealed_value(s: &Source) -> Result<Value> {
    let mut value = serde_json::to_value(s).map_err(storage_error)?;
    let fields = value.as_object_mut().ok_or(Error::Forbidden)?;
    fields.remove("ticket");
    fields.insert("ticket_sha256".into(), json!(canonical_hash(&s.ticket)));
    Ok(value)
}
pub(super) fn seal(
    keys: &dyn KeyStore,
    key: KeyId,
    s: &McpSecretScope,
    v: &Value,
) -> Result<String> {
    seal_mcp_blob(
        keys,
        key,
        s,
        &Zeroizing::new(serde_json::to_vec(v).map_err(storage_error)?),
    )
    .map_err(|_| Error::NativeKeyUnavailable)
    .and_then(|v| json_string(&v))
}
pub(super) fn open(keys: &dyn KeyStore, s: &McpSecretScope, raw: &str) -> Result<Value> {
    let b: SealedMcpBlob = parse(raw)?;
    serde_json::from_slice(&open_mcp_blob(keys, s, &b).map_err(|_| Error::NativeKeyUnavailable)?)
        .map_err(storage_error)
}
pub(super) async fn clock(tx: &mut Transaction<'_, Any>) -> Result<i64> {
    if sqlx::query("UPDATE offline_guard SET id=id WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::Forbidden);
    }
    let old: i64 = sqlx::query_scalar("SELECT clock_highwater FROM offline_guard WHERE id=1")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let t = now();
    if t < old {
        return Err(Error::WorkflowClockRollback);
    }
    sqlx::query("UPDATE offline_guard SET clock_highwater=$1 WHERE id=1")
        .bind(t)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(t)
}
pub(super) async fn configuration(
    tx: &mut Transaction<'_, Any>,
    grant: Uuid,
) -> Result<OfflineToolConfiguration> {
    parse(
        &sqlx::query_scalar::<_, String>(
            "SELECT payload FROM offline_local_tool_configs WHERE grant_id=$1",
        )
        .bind(grant.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Forbidden)?,
    )
}
pub(super) async fn current(
    tx: &mut Transaction<'_, Any>,
    s: &Source,
    write: bool,
) -> Result<OfflineRun> {
    // Capturing/reading an original response never creates time-based authority.
    // A local clock rollback still blocks every new dispatch and human release.
    let t = if write { clock(tx).await? } else { now() };
    let device: crate::offline_execution::OfflineLocalDeviceRecord = parse(
        &sqlx::query_scalar::<_, String>("SELECT payload FROM offline_local_device WHERE id=1")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )?;
    let r=sqlx::query("SELECT r.payload,r.grant_id,r.ordinal,r.revision,r.state,g.ticket,g.revoked FROM offline_local_runs r JOIN offline_local_grants g ON g.id=r.grant_id WHERE r.id=$1").bind(s.run.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let ticket: SignedOfflineGrant =
        parse(&r.try_get::<String, _>("ticket").map_err(storage_error)?)?;
    crate::offline_execution::validate_offline_ticket(&ticket)?;
    let run: OfflineRun = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    crate::offline_execution::validate_offline_output_binding(&run, &ticket.grant)?;
    let mut previous = "0".repeat(64);
    let mut stamp = run.started_at;
    for (i, j) in run.journal.iter().enumerate() {
        if j.sequence != i as u32 + 1
            || j.previous_sha256 != previous
            || j.at < stamp
            || j.sha256
                != canonical_hash(&(
                    run.id,
                    run.grant_id,
                    j.sequence,
                    j.at,
                    &j.kind,
                    &j.payload,
                    &j.previous_sha256,
                ))
        {
            return Err(Error::Forbidden);
        }
        previous = j.sha256.clone();
        stamp = j.at;
    }
    if canonical_hash(&ticket) != canonical_hash(&s.ticket)
        || device.device != ticket.grant.device
        || device.key_id.instance != device.device.id
        || run.id != s.run
        || run.device_id != device.device.id
        || run.grant_id != ticket.grant.id
        || run.ordinal != s.ordinal
        || run.deadline_at != s.deadline
        || run.ordinal == 0
        || run.ordinal > ticket.grant.maximum_runs
        || r.try_get::<i64, _>("revision").map_err(storage_error)? != run.revision
        || r.try_get::<String, _>("state").map_err(storage_error)? != run.state
        || r.try_get::<String, _>("grant_id").map_err(storage_error)? != run.grant_id.to_string()
        || r.try_get::<i64, _>("ordinal").map_err(storage_error)? != i64::from(run.ordinal)
    {
        return Err(Error::Forbidden);
    }
    if write {
        let cfg = configuration(tx, run.grant_id).await?;
        let config_sha = canonical_hash(&Some(&cfg));
        let expected = canonical_hash(&(
            ticket.grant.id,
            run.id,
            &s.tool.id,
            &s.tool.name,
            &s.tool.arguments,
            &config_sha,
        ));
        if r.try_get::<i64, _>("revoked").map_err(storage_error)? != 0
            || run.state != "running"
            || t < run.started_at
            || t >= run.deadline_at
            || t >= ticket.grant.expires_at
            || run
                .pending_tools
                .first()
                .is_none_or(|v| canonical_hash(v) != canonical_hash(&s.tool))
            || s.tool.name != "mcp_call"
            || expected != s.tool.request_sha256
            || config_sha != s.configuration_sha256
            || !ticket.grant.allowed_tools.iter().any(|v| v == "mcp_call")
            || !cfg.allowed_tools.iter().any(|v| v == "mcp_call")
        {
            return Err(Error::Forbidden);
        }
        if !run.journal.iter().any(|j| {
            j.kind == "tools_approved"
                && j.payload["request_sha256"].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|v| v.as_str() == Some(s.tool.request_sha256.as_str()))
                })
        }) || run
            .journal
            .iter()
            .filter(|j| {
                j.kind == "tool_dispatched"
                    && j.payload["request_sha256"] == s.tool.request_sha256
                    && j.payload["configuration_sha256"] == s.configuration_sha256
            })
            .count()
            != 1
        {
            return Err(Error::Forbidden);
        }
        if cfg
            .mcp_connections
            .iter()
            .find(|b| b.id == s.server_id)
            .is_none_or(|b| {
                b.endpoint != s.endpoint
                    || b.catalog_sha256 != s.catalog_sha256
                    || !b
                        .allowed_tools
                        .iter()
                        .any(|v| Some(v.as_str()) == s.tool.arguments["tool_name"].as_str())
            })
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(run)
}
pub(super) fn row_source(row: &AnyRow, keys: &dyn KeyStore) -> Result<OfflineMcpCallPermit> {
    let aad: McpSecretScope = parse(&row.try_get::<String, _>("scope").map_err(storage_error)?)?;
    let value = open(
        keys,
        &aad,
        &row.try_get::<String, _>("sealed_source")
            .map_err(storage_error)?,
    )?;
    let ticket: SignedOfflineGrant = parse(
        &row.try_get::<String, _>("original_ticket")
            .map_err(storage_error)?,
    )?;
    crate::offline_execution::validate_offline_ticket(&ticket)?;
    if value["ticket_sha256"] != canonical_hash(&ticket) {
        return Err(Error::Forbidden);
    }
    let mut full = value.clone();
    let fields = full.as_object_mut().ok_or(Error::Forbidden)?;
    fields.remove("ticket_sha256");
    fields.insert(
        "ticket".into(),
        serde_json::to_value(ticket).map_err(storage_error)?,
    );
    let s: Source = serde_json::from_value(full).map_err(storage_error)?;
    let sha: String = row.try_get("source_sha256").map_err(storage_error)?;
    if s.format != 1
        || aad != scope(&s, "task_checkpoint")
        || canonical_hash(&value) != sha
        || s.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || s.run.to_string() != row.try_get::<String, _>("run_id").map_err(storage_error)?
        || s.ticket.grant.id.to_string()
            != row
                .try_get::<String, _>("grant_id")
                .map_err(storage_error)?
        || s.ticket.grant.device.id.to_string()
            != row
                .try_get::<String, _>("device_id")
                .map_err(storage_error)?
        || s.tool.id != row.try_get::<String, _>("tool_id").map_err(storage_error)?
        || s.tool.request_sha256
            != row
                .try_get::<String, _>("tool_request_sha256")
                .map_err(storage_error)?
        || s.configuration_sha256
            != row
                .try_get::<String, _>("configuration_sha256")
                .map_err(storage_error)?
        || s.connection_id
            != row
                .try_get::<String, _>("connection_id")
                .map_err(storage_error)?
        || i64::try_from(s.connection_epoch).map_err(|_| Error::Forbidden)?
            != row
                .try_get::<i64, _>("connection_epoch")
                .map_err(storage_error)?
        || s.original_rpc_id
            != row
                .try_get::<String, _>("original_rpc_id")
                .map_err(storage_error)?
        || s.transport_request_sha256
            != row
                .try_get::<String, _>("transport_request_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(OfflineMcpCallPermit {
        source: s,
        source_sha256: sha,
    })
}
pub(super) fn facts(p: &OfflineMcpCallPermit) -> crate::mcp_scoped_tasks::Facts {
    let s = &p.source;
    crate::mcp_scoped_tasks::Facts {
        owner_kind: "offline".into(),
        owner_id: s.ticket.grant.device.id.to_string(),
        project_id: s.ticket.grant.project_id.to_string(),
        run_id: s.run.to_string(),
        call_id: s.tool.id.clone(),
        server_id: s.server_id.clone(),
        server_revision: 1,
        binding_sha256: s.configuration_sha256.clone(),
        spec_sha256: canonical_hash(&s.ticket),
        connection_id: s.connection_id.clone(),
        connection_epoch: s.connection_epoch,
        original_rpc_id: s.original_rpc_id.clone(),
        original_request_sha256: s.tool.request_sha256.clone(),
        transport_request_sha256: s.transport_request_sha256.clone(),
        authority_sha256: p.source_sha256.clone(),
    }
}
pub(super) async fn original(
    tx: &mut Transaction<'_, Any>,
    p: &OfflineMcpCallPermit,
    keys: &dyn KeyStore,
) -> Result<AnyRow> {
    let row=sqlx::query("SELECT c.*,g.ticket AS original_ticket FROM offline_mcp_calls c JOIN offline_local_grants g ON g.id=c.grant_id WHERE c.id=$1").bind(&p.source.id).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if row_source(&row, keys)?.source_sha256 != p.source_sha256 {
        return Err(Error::Forbidden);
    }
    Ok(row)
}
pub(super) async fn task_source(
    tx: &mut Transaction<'_, Any>,
    task: &McpTaskRecord,
    keys: &dyn KeyStore,
    write: bool,
) -> Result<OfflineMcpCallPermit> {
    let row=sqlx::query("SELECT c.*,g.ticket AS original_ticket FROM offline_mcp_calls c JOIN offline_local_grants g ON g.id=c.grant_id WHERE c.task_record_id=$1 AND c.run_id=$2 AND c.device_id=$3").bind(&task.view.id).bind(&task.view.run_id).bind(&task.owner_id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let p = row_source(&row, keys)?;
    crate::mcp_scoped_tasks::check(&facts(&p), task)?;
    current(tx, &p.source, write).await?;
    Ok(p)
}
pub(super) fn proof(p: &OfflineMcpCallPermit) -> Value {
    json!({"offline_device":p.source.ticket.grant.device.id,"run_id":p.source.run,"grant_id":p.source.ticket.grant.id,"tool_id":p.source.tool.id,"tool_request_sha256":p.source.tool.request_sha256,"configuration_sha256":p.source.configuration_sha256,"ticket_sha256":canonical_hash(&p.source.ticket),"source_sha256":p.source_sha256})
}
