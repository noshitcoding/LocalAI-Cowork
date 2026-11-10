use super::*;
use cowork_domain::mcp::{mcp_binding, valid_binding};
use cowork_mcp::{CheckpointScope, SealedSession};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpConnectionState {
    Connecting,
    Ready,
    Busy { tool_call_id: String },
    Unavailable { code: String },
}

/// Private encrypted continuation record. Never part of RunDetail/order exports.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpConnection {
    pub id: String,
    pub run_id: String,
    pub server_id: String,
    pub revision: i64,
    pub binding_sha256: String,
    pub state: McpConnectionState,
    pub connection_id: Option<String>,
    pub epoch: Option<u64>,
    pub checkpoint_revision: Option<i64>,
    pub sealed: Option<SealedSession>,
    owner_lease_sha256: String,
    created_at: i64,
    updated_at: i64,
}
impl McpConnection {
    pub fn next_revision(&self) -> Result<i64> {
        self.revision.checked_add(1).ok_or(Error::Conflict)
    }
    pub fn scope(&self, run: &RunRecord, checkpoint_revision: i64) -> Result<CheckpointScope> {
        let binding = server_binding(run, &self.server_id)?;
        if self.run_id != run.id
            || self.binding_sha256 != binding_hash(binding)
            || checkpoint_revision < 1
        {
            return Err(Error::Forbidden);
        }
        Ok(CheckpointScope {
            project_id: run.spec.project_id.clone(),
            run_id: run.id.clone(),
            server_id: self.server_id.clone(),
            connection_record_id: self.id.clone(),
            server_revision: binding.server_revision,
            checkpoint_revision,
            configuration_sha256: binding.configuration_sha256.clone(),
            catalog_sha256: binding.catalog_sha256.clone(),
        })
    }
    fn validate(&self, run: &RunRecord) -> Result<()> {
        for s in [&self.id, &self.run_id, &self.server_id] {
            valid_text(s, 128)?;
        }
        if self.revision < 1
            || self.created_at <= 0
            || self.updated_at < self.created_at
            || !cowork_domain::processes::valid_sha256(&self.owner_lease_sha256)
        {
            return Err(Error::Forbidden);
        }
        self.scope(run, self.revision)?;
        let complete = self
            .connection_id
            .as_ref()
            .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|v| !v.is_nil()))
            && self.epoch.is_some_and(|n| n < u64::MAX)
            && self
                .checkpoint_revision
                .is_some_and(|r| r > 1 && r <= self.revision)
            && self.sealed.as_ref().is_some_and(|s| s.validate().is_ok());
        let empty = self.connection_id.is_none()
            && self.epoch.is_none()
            && self.checkpoint_revision.is_none()
            && self.sealed.is_none();
        match &self.state {
            McpConnectionState::Connecting if self.revision == 1 && empty => {}
            McpConnectionState::Ready
                if complete && self.checkpoint_revision == Some(self.revision) => {}
            McpConnectionState::Busy { tool_call_id } if complete => {
                valid_text(tool_call_id, 128)?;
            }
            McpConnectionState::Unavailable { code }
                if (complete || empty) && self.revision > 1 =>
            {
                valid_text(code, 128)?;
                if !code.bytes().all(|c| c.is_ascii_lowercase() || c == b'_') {
                    return Err(Error::Forbidden);
                }
            }
            _ => return Err(Error::Forbidden),
        }
        Ok(())
    }
}
pub struct McpConnectionPermit {
    record: McpConnection,
}
impl McpConnectionPermit {
    pub fn record(&self) -> &McpConnection {
        &self.record
    }
}
pub enum McpConnectionAdmission {
    Connect(McpConnectionPermit),
    Existing(McpConnection),
}
#[derive(Clone)]
pub(super) struct ConnectionWrite {
    record: McpConnection,
    expected_revision: Option<i64>,
}
#[derive(Clone)]
pub enum McpSessionCompletion {
    Ready {
        record_id: String,
        expected_revision: i64,
        connection_id: String,
        epoch: u64,
        sealed: SealedSession,
    },
    Unavailable {
        code: String,
    },
}

fn server_binding<'a>(run: &'a RunRecord, server: &str) -> Result<&'a McpToolBinding> {
    let mut bindings = run.spec.mcp_tools.iter().filter(|b| b.server_id == server);
    let first = bindings.next().ok_or(Error::Forbidden)?;
    if !valid_binding(first)
        || bindings.any(|b| !valid_binding(b) || binding_hash(b) != binding_hash(first))
    {
        return Err(Error::Forbidden);
    }
    Ok(first)
}
fn binding_hash(binding: &McpToolBinding) -> String {
    canonical_hash(&json!([
        binding.server_id,
        binding.server_revision,
        binding.endpoint,
        binding.configuration_sha256,
        binding.catalog_sha256
    ]))
}
pub(super) fn checked(row: &sqlx::any::AnyRow, run: &RunRecord) -> Result<McpConnection> {
    let record: McpConnection =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    record.validate(run)?;
    if record.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || record.server_id
            != row
                .try_get::<String, _>("server_id")
                .map_err(storage_error)?
        || record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(record)
}
async fn load(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    server: &str,
) -> Result<Option<McpConnection>> {
    sqlx::query("SELECT id,server_id,revision,payload FROM mcp_connections WHERE run_id=$1 AND server_id=$2")
        .bind(&run.id).bind(server).fetch_optional(&mut **tx).await.map_err(storage_error)?
        .as_ref().map(|row|checked(row,run)).transpose()
}
async fn update(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    record: &McpConnection,
    previous: i64,
) -> Result<()> {
    record.validate(run)?;
    if let Some(sealed) = &record.sealed {
        crate::native_key_retirement::allow_cipher(tx, sealed.key_id()).await?;
    }
    if record.revision != previous.checked_add(1).ok_or(Error::Conflict)? {
        return Err(Error::Conflict);
    }
    let changed=sqlx::query("UPDATE mcp_connections SET revision=$1,payload=$2 WHERE id=$3 AND run_id=$4 AND revision=$5")
        .bind(record.revision).bind(json_string(record)?).bind(&record.id).bind(&run.id).bind(previous)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
    if changed != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
impl Store {
    pub async fn mcp_connection(&self, run: &str, server: &str) -> Result<Option<McpConnection>> {
        let stored = self.stored_run(run).await?;
        let row=sqlx::query("SELECT id,server_id,revision,payload FROM mcp_connections WHERE run_id=$1 AND server_id=$2")
            .bind(run).bind(server).fetch_optional(&self.pool).await.map_err(storage_error)?;
        row.as_ref()
            .map(|row| checked(row, &stored.record))
            .transpose()
    }
    pub async fn begin_mcp_connection(
        &self,
        run: &str,
        token: &str,
        server: &str,
    ) -> Result<McpConnectionAdmission> {
        let stored = self.stored_run(run).await?;
        stored.assert_lease(token, now())?;
        let binding = server_binding(&stored.record, server)?;
        if let Some(record) = self.mcp_connection(run, server).await? {
            return Ok(McpConnectionAdmission::Existing(record));
        }
        let record = McpConnection {
            id: id(),
            run_id: run.into(),
            server_id: server.into(),
            revision: 1,
            binding_sha256: binding_hash(binding),
            state: McpConnectionState::Connecting,
            connection_id: None,
            epoch: None,
            checkpoint_revision: None,
            sealed: None,
            owner_lease_sha256: hash(token.as_bytes()),
            created_at: now(),
            updated_at: now(),
        };
        let commit = self
            .mutate(run, Some(token), |r| {
                if r.record.state != RunState::Running || r.checkpoint.in_flight.is_some() {
                    return Err(Error::Conflict);
                }
                if time_budget_exhausted(&r.record, now()) {
                    return Err(Error::BudgetExceeded);
                }
                let mut change = Change::event(
                    "mcp_connection_reserved",
                    json!({"connection_record_id":record.id,"server_id":server}),
                );
                change.mcp_connection_write = Some(ConnectionWrite {
                    record: record.clone(),
                    expected_revision: None,
                });
                Ok(change)
            })
            .await;
        match commit {
            Ok(_) => Ok(McpConnectionAdmission::Connect(McpConnectionPermit {
                record,
            })),
            Err(e) => match self.mcp_connection(run, server).await? {
                Some(record) => Ok(McpConnectionAdmission::Existing(record)),
                None => Err(e),
            },
        }
    }
    pub async fn finish_mcp_connection(
        &self,
        run: &str,
        token: &str,
        permit: McpConnectionPermit,
        connection_id: String,
        epoch: u64,
        sealed: SealedSession,
    ) -> Result<McpConnection> {
        let mut record = permit.record;
        if record.run_id != run || record.owner_lease_sha256 != hash(token.as_bytes()) {
            return Err(Error::StaleLease);
        }
        record.revision = 2;
        record.state = McpConnectionState::Ready;
        record.updated_at = now();
        record.connection_id = Some(connection_id);
        record.epoch = Some(epoch);
        record.checkpoint_revision = Some(2);
        record.sealed = Some(sealed);
        let commit = self
            .mutate(run, Some(token), |r| {
                if r.record.state != RunState::Running || r.checkpoint.in_flight.is_some() {
                    return Err(Error::Conflict);
                }
                let mut change = Change::event(
                    "mcp_connection_ready",
                    json!({"connection_record_id":record.id,"server_id":record.server_id}),
                );
                change.mcp_connection_write = Some(ConnectionWrite {
                    record: record.clone(),
                    expected_revision: Some(1),
                });
                Ok(change)
            })
            .await;
        match commit {
            Ok(_) => Ok(record),
            Err(e) => match self.mcp_connection(run, &record.server_id).await? {
                Some(saved) if saved == record => Ok(saved),
                _ => Err(e),
            },
        }
    }
}
pub(super) async fn write(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &StoredRun,
    write: &ConnectionWrite,
) -> Result<()> {
    let record = &write.record;
    record.validate(&run.record)?;
    let binding = server_binding(&run.record, &record.server_id)?;
    mcp_servers::check_binding(tx, &run.record, binding).await?;
    if run.record.state != RunState::Running
        || run.checkpoint.in_flight.is_some()
        || run
            .lease
            .as_ref()
            .map(|l| hash(l.token.as_bytes()))
            .as_ref()
            != Some(&record.owner_lease_sha256)
    {
        return Err(Error::StaleLease);
    }
    match write.expected_revision {
        None => {
            if record.state != McpConnectionState::Connecting {
                return Err(Error::Forbidden);
            }
            let changed=sqlx::query("INSERT INTO mcp_connections(id,run_id,server_id,revision,payload) VALUES($1,$2,$3,1,$4) ON CONFLICT(run_id,server_id) DO NOTHING")
                .bind(&record.id).bind(&record.run_id).bind(&record.server_id).bind(json_string(record)?)
                .execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
            if changed != 1 {
                return Err(Error::Conflict);
            }
            Ok(())
        }
        Some(1) => {
            let mut old = load(tx, &run.record, &record.server_id)
                .await?
                .ok_or(Error::NotFound)?;
            if old.state != McpConnectionState::Connecting
                || old.revision != 1
                || record.state != McpConnectionState::Ready
            {
                return Err(Error::Conflict);
            }
            old.state = record.state.clone();
            old.revision = 2;
            old.updated_at = record.updated_at;
            old.connection_id = record.connection_id.clone();
            old.epoch = record.epoch;
            old.checkpoint_revision = Some(2);
            old.sealed = record.sealed.clone();
            if old != *record {
                return Err(Error::Forbidden);
            }
            update(tx, &run.record, record, 1).await
        }
        _ => Err(Error::Forbidden),
    }
}
/// Called inside the same transaction as the exact tool dispatch marker.
pub(super) async fn admit(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    attempt: &McpAttempt,
    epoch: u64,
) -> Result<()> {
    let Some(mut connection) = load(tx, run, &attempt.binding.server_id).await? else {
        return Ok(());
    };
    if connection.state != McpConnectionState::Ready
        || connection.connection_id.as_ref() != Some(&attempt.connection_id)
        || connection.epoch != Some(epoch)
        || binding_hash(mcp_binding(run, &attempt.tool)?) != connection.binding_sha256
    {
        return Err(Error::Conflict);
    }
    let revision = connection.revision;
    connection.revision = connection.next_revision()?;
    connection.updated_at = now();
    connection.state = McpConnectionState::Busy {
        tool_call_id: attempt.tool.id.clone(),
    };
    update(tx, run, &connection, revision).await
}
/// Completion/checkpoint and uncertain invalidation share the tool journal transaction.
pub(super) async fn settle(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    attempt: &McpAttempt,
    completion: Option<&McpSessionCompletion>,
) -> Result<()> {
    let Some(mut connection) = load(tx, run, &attempt.binding.server_id).await? else {
        return if completion.is_none() {
            Ok(())
        } else {
            Err(Error::Forbidden)
        };
    };
    if connection.connection_id.as_ref() != Some(&attempt.connection_id) {
        return Err(Error::Forbidden);
    }
    let revision = connection.revision;
    match &connection.state {
        McpConnectionState::Busy { tool_call_id } if tool_call_id == &attempt.tool.id => {}
        McpConnectionState::Unavailable { .. }
            if matches!(
                attempt.state,
                McpAttemptState::ReconciledCompleted { .. }
                    | McpAttemptState::ReconciledNotExecuted { .. }
            ) => {}
        _ => return Err(Error::Conflict),
    }
    connection.revision = connection.next_revision()?;
    connection.updated_at = now();
    if matches!(attempt.state, McpAttemptState::Completed { .. }) {
        match completion.ok_or(Error::Forbidden)? {
            McpSessionCompletion::Ready {
                record_id,
                expected_revision,
                connection_id,
                epoch,
                sealed,
            } => {
                if record_id != &connection.id
                    || *expected_revision != revision
                    || Some(connection_id) != connection.connection_id.as_ref()
                    || Some(*epoch) != connection.epoch.and_then(|e| e.checked_add(1))
                {
                    return Err(Error::Conflict);
                }
                connection.epoch = Some(*epoch);
                connection.sealed = Some(sealed.clone());
                connection.checkpoint_revision = Some(connection.revision);
                connection.state = McpConnectionState::Ready;
            }
            McpSessionCompletion::Unavailable { code } => {
                connection.state = McpConnectionState::Unavailable { code: code.clone() }
            }
        }
    } else {
        if completion.is_some() {
            return Err(Error::Forbidden);
        }
        connection.state = McpConnectionState::Unavailable {
            code: "mcp_outcome_requires_review".into(),
        };
    }
    update(tx, run, &connection, revision).await
}
