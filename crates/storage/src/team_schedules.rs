//! Explicit durable team scheduler grants, separate from expiring login sessions.
//! No replay of provider/tool effects; each occurrence has one reserved run UUID.
use super::*;
use crate::team_service::{TeamService, identity_error};
use cowork_identity::{ContentAction, ContentPermit, Principal};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction, any::AnyRow};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamScheduleTemplate {
    #[serde(default)]
    pub plan: TeamRunPlan,
    pub order_id: Uuid,
    pub prompt: String,
    pub project_revision: i64,
    pub order_revision: i64,
    pub provider_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSchedule {
    pub id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub timezone: String,
    pub recurrence: ScheduleRecurrence,
    pub template: TeamScheduleTemplate,
    pub paused: bool,
    pub next_run: Option<i64>,
    pub active_occurrence_id: Option<Uuid>,
    pub last_run_id: Option<Uuid>,
    pub last_error: Option<String>,
    pub revision: i64,
    pub execution_revision: i64,
    pub grant_identity_id: Uuid,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteTeamSchedule {
    pub id: Option<Uuid>,
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub name: String,
    pub timezone: String,
    pub recurrence: ScheduleRecurrence,
    pub template: TeamScheduleTemplate,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub grant_execution: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseTeamSchedule {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub paused: bool,
    #[serde(default)]
    pub grant_execution: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteTeamSchedule {
    pub expected_revision: i64,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamScheduleDeleted {
    pub id: Uuid,
    pub revision: i64,
    pub deleted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamScheduleOccurrence {
    pub id: Uuid,
    pub sequence: i64,
    pub due_at: i64,
    pub claimed_at: i64,
    pub run_id: Option<Uuid>,
    pub status: String,
    pub error: Option<String>,
    pub completed_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamScheduleHistory {
    pub occurrences: Vec<TeamScheduleOccurrence>,
    pub next_cursor: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GrantScope {
    identity_id: Uuid,
    identity_revision: i64,
    organization_epoch: i64,
    membership_id: String,
    membership_revision: i64,
    resource_revision: i64,
    content_grant_revision: i64,
    project_revision: i64,
    order_revision: i64,
    provider_revision: i64,
}
struct ScheduleRow {
    value: TeamSchedule,
    grant_id: Uuid,
    deleted: bool,
}
struct OccurrenceRow {
    value: TeamScheduleOccurrence,
    schedule_id: Uuid,
    team_id: Uuid,
    project_id: Uuid,
    execution_revision: i64,
    grant_id: Uuid,
    reserved_run_id: Uuid,
    creation_key: String,
    request: CreateTeamRun,
}
/// Only the persisted claim operation can mint this. Never deserialize authority.
pub struct TeamScheduleDispatch {
    occurrence_id: Uuid,
    schedule_id: Uuid,
}
pub(crate) struct TeamScheduleRunPermit {
    pub(crate) actor: Principal,
    pub(crate) run_id: Uuid,
    pub(crate) team_id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) project_revision: i64,
    pub(crate) order_revision: i64,
    pub(crate) provider_revision: i64,
    pub(crate) input: CreateTeamRun,
    pub(crate) occurrence_id: Uuid,
    pub(crate) schedule_id: Uuid,
    pub(crate) grant_id: Uuid,
}
pub(crate) enum TeamSchedulePreparation {
    Existing(Box<TeamRun>),
    New(Box<TeamScheduleRunPermit>),
}

fn invalid() -> Error {
    Error::Validation("Invalid team schedule or explicit execution grant".into())
}
fn uuid(value: String) -> Result<Uuid> {
    Uuid::parse_str(&value).map_err(storage_error)
}
fn intent(value: &str) -> Result<()> {
    valid_text(value, 128)?;
    if value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
async fn clock(store: &Store, tx: &mut Transaction<'_, Any>) -> Result<i64> {
    let query = if store.sqlite {
        "SELECT CAST((julianday('now')-2440587.5)*86400000 AS BIGINT)"
    } else {
        "SELECT CAST(EXTRACT(EPOCH FROM clock_timestamp())*1000 AS BIGINT)"
    };
    sqlx::query_scalar(query)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)
}
async fn lock(store: &Store, tx: &mut Transaction<'_, Any>) -> Result<i64> {
    let identity = sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    if identity.rows_affected() != 1 {
        return Err(Error::Forbidden);
    }
    let guard = sqlx::query("UPDATE team_schedule_guard SET sequence=sequence WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    if guard.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    let highwater: i64 =
        sqlx::query_scalar("SELECT clock_highwater FROM team_schedule_guard WHERE id=1")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let stamp = clock(store, tx).await?;
    if stamp < highwater {
        return Err(Error::WorkflowClockRollback);
    }
    sqlx::query("UPDATE team_schedule_guard SET clock_highwater=$1 WHERE id=1")
        .bind(stamp)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(stamp)
}
async fn authorize(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    team: Uuid,
    project: Uuid,
    action: ContentAction,
) -> Result<ContentPermit> {
    let permit = store
        .identity()
        .authorize_content_in_transaction(tx, actor, team, project, action)
        .await
        .map_err(identity_error)?;
    let row=sqlx::query("SELECT p.organization_id,COALESCE(f.archived,0) AS archived FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.id=$1").bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if row
        .try_get::<String, _>("organization_id")
        .map_err(storage_error)?
        != team.to_string()
    {
        return Err(Error::Forbidden);
    }
    let archived: i64 = row.try_get("archived").map_err(storage_error)?;
    if !matches!(archived, 0 | 1) || archived == 1 && action != ContentAction::Read {
        return Err(Error::Conflict);
    }
    Ok(permit)
}
async fn finish_auth(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    permit: &ContentPermit,
) -> Result<()> {
    store
        .identity()
        .validate_content_in_transaction(tx, permit)
        .await
        .map_err(identity_error)
}
async fn scope(
    tx: &mut Transaction<'_, Any>,
    identity: Uuid,
    team: Uuid,
    project: Uuid,
    order_id: Uuid,
) -> Result<GrantScope> {
    let row=sqlx::query("SELECT i.revision AS identity_revision,o.authorization_epoch,m.id AS membership_id,m.revision AS membership_revision,r.revision AS resource_revision,g.revision AS content_grant_revision,COALESCE(f.revision,1) AS project_revision,p.revision AS provider_revision,t.organization_id,COALESCE(f.archived,0) AS archived FROM identity_subjects i JOIN identity_memberships m ON m.identity_id=i.id AND m.organization_id=$2 JOIN identity_organizations o ON o.id=m.organization_id JOIN identity_resources r ON r.id=$3 AND r.organization_id=o.id JOIN identity_content_grants g ON g.resource_id=r.id AND g.membership_id=m.id JOIN tenant_projects t ON t.id=r.id LEFT JOIN team_project_profiles f ON f.project_id=t.id JOIN team_provider p ON p.team_id=o.id WHERE i.id=$1 AND i.active=1 AND o.state='active' AND m.active=1 AND m.role<>'viewer' AND g.active=1 AND (g.permissions&5)=5")
        .bind(identity.to_string()).bind(team.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if row
        .try_get::<String, _>("organization_id")
        .map_err(storage_error)?
        != team.to_string()
        || row.try_get::<i64, _>("archived").map_err(storage_error)? != 0
    {
        return Err(Error::Forbidden);
    }
    let order: TeamOrder = parse(
        &sqlx::query(
            "SELECT payload FROM team_orders WHERE id=$1 AND team_id=$2 AND project_id=$3",
        )
        .bind(order_id.to_string())
        .bind(team.to_string())
        .bind(project.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?
        .try_get::<String, _>("payload")
        .map_err(storage_error)?,
    )?;
    if order.id != order_id
        || order.team_id != team
        || order.project_id != project
        || order.archived
    {
        return Err(Error::Conflict);
    }
    Ok(GrantScope {
        identity_id: identity,
        identity_revision: row.try_get("identity_revision").map_err(storage_error)?,
        organization_epoch: row.try_get("authorization_epoch").map_err(storage_error)?,
        membership_id: row.try_get("membership_id").map_err(storage_error)?,
        membership_revision: row.try_get("membership_revision").map_err(storage_error)?,
        resource_revision: row.try_get("resource_revision").map_err(storage_error)?,
        content_grant_revision: row
            .try_get("content_grant_revision")
            .map_err(storage_error)?,
        project_revision: row.try_get("project_revision").map_err(storage_error)?,
        order_revision: order.revision,
        provider_revision: row.try_get("provider_revision").map_err(storage_error)?,
    })
}
fn check_template(value: &TeamScheduleTemplate, grant: &GrantScope) -> Result<()> {
    if value.project_revision != grant.project_revision
        || value.order_revision != grant.order_revision
        || value.provider_revision != grant.provider_revision
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn read_schedule(
    tx: &mut Transaction<'_, Any>,
    team: Uuid,
    project: Uuid,
    key: Uuid,
) -> Result<ScheduleRow> {
    let row =
        sqlx::query("SELECT * FROM team_schedules WHERE id=$1 AND team_id=$2 AND project_id=$3")
            .bind(key.to_string())
            .bind(team.to_string())
            .bind(project.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    decode_schedule(&row)
}
fn decode_schedule(row: &AnyRow) -> Result<ScheduleRow> {
    let value: TeamSchedule = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let grant_id = uuid(row.try_get("grant_id").map_err(storage_error)?)?;
    let active: Option<String> = row.try_get("active_occurrence_id").map_err(storage_error)?;
    let paused: i64 = row.try_get("paused").map_err(storage_error)?;
    let deleted: i64 = row.try_get("deleted").map_err(storage_error)?;
    let indexed_next: i64 = row.try_get("next_run").map_err(storage_error)?;
    let expected_next = if value.active_occurrence_id.is_some() {
        i64::MAX
    } else {
        value.next_run.unwrap_or(i64::MAX)
    };
    if value.id.to_string() != row.try_get::<String, _>("id").map_err(storage_error)?
        || value.team_id.to_string()
            != row.try_get::<String, _>("team_id").map_err(storage_error)?
        || value.project_id.to_string()
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || value.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || value.execution_revision
            != row
                .try_get::<i64, _>("execution_revision")
                .map_err(storage_error)?
        || i64::from(value.paused) != paused
        || value.active_occurrence_id.map(|v| v.to_string()) != active
        || !matches!(deleted, 0 | 1)
        || value.revision <= 0
        || value.execution_revision <= 0
        || value.execution_revision > value.revision
        || indexed_next != expected_next
    {
        return Err(Error::Conflict);
    }
    Ok(ScheduleRow {
        value,
        grant_id,
        deleted: deleted == 1,
    })
}
async fn save(tx: &mut Transaction<'_, Any>, row: &ScheduleRow) -> Result<()> {
    let v = &row.value;
    sqlx::query("INSERT INTO team_schedules(id,team_id,project_id,revision,execution_revision,grant_id,paused,deleted,next_run,active_occurrence_id,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,execution_revision=excluded.execution_revision,grant_id=excluded.grant_id,paused=excluded.paused,deleted=excluded.deleted,next_run=excluded.next_run,active_occurrence_id=excluded.active_occurrence_id,payload=excluded.payload WHERE team_schedules.team_id=excluded.team_id AND team_schedules.project_id=excluded.project_id")
        .bind(v.id.to_string()).bind(v.team_id.to_string()).bind(v.project_id.to_string()).bind(v.revision).bind(v.execution_revision).bind(row.grant_id.to_string()).bind(i64::from(v.paused)).bind(i64::from(row.deleted)).bind(if v.active_occurrence_id.is_some(){i64::MAX}else{v.next_run.unwrap_or(i64::MAX)}).bind(v.active_occurrence_id.map(|v|v.to_string())).bind(json_string(v)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn install_grant(
    tx: &mut Transaction<'_, Any>,
    row: &ScheduleRow,
    grant: &GrantScope,
) -> Result<()> {
    sqlx::query("UPDATE team_schedule_grants SET active=0 WHERE schedule_id=$1")
        .bind(row.value.id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    sqlx::query("INSERT INTO team_schedule_grants(id,schedule_id,team_id,project_id,identity_id,execution_revision,active,payload,created_at) VALUES($1,$2,$3,$4,$5,$6,1,$7,$8)").bind(row.grant_id.to_string()).bind(row.value.id.to_string()).bind(row.value.team_id.to_string()).bind(row.value.project_id.to_string()).bind(grant.identity_id.to_string()).bind(row.value.execution_revision).bind(json_string(grant)?).bind(row.value.updated_at).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn validate_grant(tx: &mut Transaction<'_, Any>, row: &ScheduleRow) -> Result<GrantScope> {
    if row.deleted {
        return Err(Error::Forbidden);
    }
    let grant=sqlx::query("SELECT payload,identity_id FROM team_schedule_grants WHERE id=$1 AND schedule_id=$2 AND team_id=$3 AND project_id=$4 AND execution_revision=$5 AND active=1").bind(row.grant_id.to_string()).bind(row.value.id.to_string()).bind(row.value.team_id.to_string()).bind(row.value.project_id.to_string()).bind(row.value.execution_revision).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let captured: GrantScope = parse(
        &grant
            .try_get::<String, _>("payload")
            .map_err(storage_error)?,
    )?;
    if captured.identity_id.to_string()
        != grant
            .try_get::<String, _>("identity_id")
            .map_err(storage_error)?
        || captured.identity_id != row.value.grant_identity_id
    {
        return Err(Error::Forbidden);
    }
    let actual = scope(
        tx,
        captured.identity_id,
        row.value.team_id,
        row.value.project_id,
        row.value.template.order_id,
    )
    .await?;
    if actual != captured {
        return Err(Error::Forbidden);
    }
    check_template(&row.value.template, &actual)?;
    Ok(actual)
}
async fn replay<T: serde::de::DeserializeOwned>(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    team: Uuid,
    project: Uuid,
    key: &str,
    digest: &str,
) -> Result<Option<T>> {
    let row=sqlx::query("SELECT project_id,request_sha,receipt FROM team_schedule_operations WHERE actor_id=$1 AND team_id=$2 AND intent_key=$3").bind(actor.identity_id().to_string()).bind(team.to_string()).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    match row {
        None => Ok(None),
        Some(row) => {
            if row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
                != project.to_string()
                || row
                    .try_get::<String, _>("request_sha")
                    .map_err(storage_error)?
                    != digest
            {
                return Err(Error::Conflict);
            }
            Ok(Some(parse(
                &row.try_get::<String, _>("receipt").map_err(storage_error)?,
            )?))
        }
    }
}
async fn remember<T: Serialize>(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    team: Uuid,
    project: Uuid,
    key: &str,
    digest: &str,
    value: &T,
) -> Result<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM team_schedule_operations WHERE team_id=$1")
            .bind(team.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count >= 100_000 {
        return Err(Error::Conflict);
    }
    sqlx::query("INSERT INTO team_schedule_operations(actor_id,team_id,project_id,intent_key,request_sha,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(actor.identity_id().to_string()).bind(team.to_string()).bind(project.to_string()).bind(key).bind(digest).bind(json_string(value)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

impl Store {
    pub async fn team_schedules(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
    ) -> Result<Vec<TeamSchedule>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT * FROM team_schedules WHERE team_id=$1 AND project_id=$2 AND deleted=0 ORDER BY id LIMIT 501").bind(team.to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| decode_schedule(r).map(|r| r.value))
            .collect::<Result<Vec<_>>>()?;
        finish_auth(self, &mut tx, &permit).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn team_write_schedule(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        input: WriteTeamSchedule,
    ) -> Result<TeamSchedule> {
        intent(&input.idempotency_key)?;
        valid_text(&input.name, 200)?;
        valid_text(&input.template.prompt, 128 * 1024)?;
        if !input.grant_execution
            || input.template.project_revision <= 0
            || input.template.order_revision <= 0
            || input.template.provider_revision <= 0
        {
            return Err(invalid());
        }
        let digest = hash(json_string(&("write", team, project, &input))?.as_bytes());
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Write).await?;
        authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        let execute =
            authorize(self, &mut tx, actor, team, project, ContentAction::Execute).await?;
        let stamp = lock(self, &mut tx).await?;
        if let Some(value) = replay(
            &mut tx,
            actor,
            team,
            project,
            &input.idempotency_key,
            &digest,
        )
        .await?
        {
            finish_auth(self, &mut tx, &permit).await?;
            finish_auth(self, &mut tx, &execute).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(value);
        }
        let previous = match input.id {
            Some(id) => Some(read_schedule(&mut tx, team, project, id).await?),
            None => None,
        };
        if previous.as_ref().is_some_and(|r| {
            r.deleted
                || r.value.revision != input.expected_revision
                || r.value.active_occurrence_id.is_some()
        }) || previous.is_none() && input.expected_revision != 0
        {
            return Err(Error::Conflict);
        }
        if previous.is_none() {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_schedules WHERE team_id=$1 AND project_id=$2 AND deleted=0").bind(team.to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count >= 500 {
                return Err(Error::Conflict);
            }
        }
        let grant = scope(
            &mut tx,
            actor.identity_id(),
            team,
            project,
            input.template.order_id,
        )
        .await?;
        check_template(&input.template, &grant)?;
        let next = productivity_next_run(&input.recurrence, &input.timezone, stamp)?;
        if next.is_none() && !input.paused {
            return Err(invalid());
        }
        let revision = previous
            .as_ref()
            .map(|r| r.value.revision)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        let row = ScheduleRow {
            grant_id: Uuid::new_v4(),
            deleted: false,
            value: TeamSchedule {
                id: previous
                    .as_ref()
                    .map(|r| r.value.id)
                    .unwrap_or_else(Uuid::new_v4),
                team_id: team,
                project_id: project,
                name: input.name,
                timezone: input.timezone,
                recurrence: input.recurrence,
                template: input.template,
                paused: input.paused,
                next_run: next,
                active_occurrence_id: None,
                last_run_id: previous.as_ref().and_then(|r| r.value.last_run_id),
                last_error: None,
                revision,
                execution_revision: revision,
                grant_identity_id: actor.identity_id(),
                created_at: previous
                    .as_ref()
                    .map(|r| r.value.created_at)
                    .unwrap_or(stamp),
                updated_at: stamp,
            },
        };
        save(&mut tx, &row).await?;
        install_grant(&mut tx, &row, &grant).await?;
        remember(
            &mut tx,
            actor,
            team,
            project,
            &input.idempotency_key,
            &digest,
            &row.value,
        )
        .await?;
        finish_auth(self, &mut tx, &permit).await?;
        finish_auth(self, &mut tx, &execute).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(row.value)
    }
    pub async fn team_pause_schedule(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        key: Uuid,
        input: PauseTeamSchedule,
    ) -> Result<TeamSchedule> {
        intent(&input.idempotency_key)?;
        if !input.paused && !input.grant_execution {
            return Err(invalid());
        }
        let digest = hash(json_string(&("pause", team, project, key, &input))?.as_bytes());
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Write).await?;
        let execute = if input.paused {
            None
        } else {
            Some(authorize(self, &mut tx, actor, team, project, ContentAction::Execute).await?)
        };
        let stamp = lock(self, &mut tx).await?;
        if let Some(value) = replay(
            &mut tx,
            actor,
            team,
            project,
            &input.idempotency_key,
            &digest,
        )
        .await?
        {
            finish_auth(self, &mut tx, &permit).await?;
            if let Some(p) = &execute {
                finish_auth(self, &mut tx, p).await?;
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(value);
        }
        let mut row = read_schedule(&mut tx, team, project, key).await?;
        if row.deleted
            || row.value.revision != input.expected_revision
            || !input.paused && row.value.active_occurrence_id.is_some()
        {
            return Err(Error::Conflict);
        }
        row.value.revision = row.value.revision.checked_add(1).ok_or(Error::Conflict)?;
        row.value.paused = input.paused;
        row.value.updated_at = stamp;
        if !input.paused {
            let grant = scope(
                &mut tx,
                actor.identity_id(),
                team,
                project,
                row.value.template.order_id,
            )
            .await?;
            check_template(&row.value.template, &grant)?;
            row.grant_id = Uuid::new_v4();
            row.value.execution_revision = row.value.revision;
            row.value.grant_identity_id = actor.identity_id();
            row.value.next_run =
                productivity_next_run(&row.value.recurrence, &row.value.timezone, stamp)?;
            row.value.last_error = None;
            if row.value.next_run.is_none() {
                return Err(invalid());
            }
            save(&mut tx, &row).await?;
            install_grant(&mut tx, &row, &grant).await?;
        } else {
            save(&mut tx, &row).await?;
        }
        remember(
            &mut tx,
            actor,
            team,
            project,
            &input.idempotency_key,
            &digest,
            &row.value,
        )
        .await?;
        finish_auth(self, &mut tx, &permit).await?;
        if let Some(p) = &execute {
            finish_auth(self, &mut tx, p).await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(row.value)
    }
    pub async fn team_delete_schedule(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        key: Uuid,
        input: DeleteTeamSchedule,
    ) -> Result<TeamScheduleDeleted> {
        intent(&input.idempotency_key)?;
        let digest = hash(json_string(&("delete", team, project, key, &input))?.as_bytes());
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Write).await?;
        let stamp = lock(self, &mut tx).await?;
        if let Some(value) = replay(
            &mut tx,
            actor,
            team,
            project,
            &input.idempotency_key,
            &digest,
        )
        .await?
        {
            finish_auth(self, &mut tx, &permit).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(value);
        }
        let mut row = read_schedule(&mut tx, team, project, key).await?;
        if row.deleted
            || row.value.revision != input.expected_revision
            || row.value.active_occurrence_id.is_some()
        {
            return Err(Error::Conflict);
        }
        row.deleted = true;
        row.value.paused = true;
        row.value.next_run = None;
        row.value.updated_at = stamp;
        row.value.revision = row.value.revision.checked_add(1).ok_or(Error::Conflict)?;
        save(&mut tx, &row).await?;
        sqlx::query("UPDATE team_schedule_grants SET active=0 WHERE schedule_id=$1")
            .bind(key.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = TeamScheduleDeleted {
            id: key,
            revision: row.value.revision,
            deleted: true,
        };
        remember(
            &mut tx,
            actor,
            team,
            project,
            &input.idempotency_key,
            &digest,
            &result,
        )
        .await?;
        finish_auth(self, &mut tx, &permit).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn team_schedule_history(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        key: Uuid,
        cursor: i64,
        limit: i64,
    ) -> Result<TeamScheduleHistory> {
        if cursor < 0 || !(1..=100).contains(&limit) {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        read_schedule(&mut tx, team, project, key).await?;
        let rows=sqlx::query("SELECT * FROM team_schedule_occurrences WHERE schedule_id=$1 AND team_id=$2 AND project_id=$3 AND sequence>$4 ORDER BY sequence LIMIT $5").bind(key.to_string()).bind(team.to_string()).bind(project.to_string()).bind(cursor).bind(limit+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let values = rows
            .iter()
            .take(limit as usize)
            .map(|r| decode_occurrence(r).map(|r| r.value))
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = if more {
            values.last().map(|v| v.sequence)
        } else {
            None
        };
        finish_auth(self, &mut tx, &permit).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamScheduleHistory {
            occurrences: values,
            next_cursor,
        })
    }
    /// A worker calls this; it owns no user bearer and derives each dispatch from SQL.
    pub async fn team_schedule_tick(&self, teams: &TeamService) -> Result<usize> {
        self.reconcile_team_schedules().await?;
        let mut count = 0;
        for _ in 0..8 {
            let Some(dispatch) = self.claim_team_schedule().await? else {
                break;
            };
            let occurrence = dispatch.occurrence_id;
            if let Err(error) = teams.create_scheduled_run(dispatch).await {
                let definitely_rejected = matches!(
                    error,
                    Error::Forbidden | Error::Conflict | Error::NotFound | Error::Validation(_)
                );
                self.note_team_schedule_admission_failure(occurrence, definitely_rejected)
                    .await?;
            }
            count += 1;
        }
        Ok(count)
    }
    async fn claim_team_schedule(&self) -> Result<Option<TeamScheduleDispatch>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = lock(self, &mut tx).await?;
        let rows=sqlx::query("SELECT * FROM team_schedules WHERE deleted=0 AND paused=0 AND active_occurrence_id IS NULL AND next_run<=$1 ORDER BY next_run,id LIMIT 16").bind(stamp).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for value in rows {
            let mut row = decode_schedule(&value)?;
            if validate_grant(&mut tx, &row).await.is_err() {
                row.value.paused = true;
                row.value.last_error = Some("authorization_or_revision_changed".into());
                row.value.updated_at = stamp;
                save(&mut tx, &row).await?;
                continue;
            }
            let history: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_schedule_occurrences WHERE schedule_id=$1",
            )
            .bind(row.value.id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if history >= 10_000 {
                row.value.paused = true;
                row.value.last_error = Some("schedule_history_limit".into());
                save(&mut tx, &row).await?;
                continue;
            }
            let sequence: i64 =
                sqlx::query_scalar("SELECT sequence FROM team_schedule_guard WHERE id=1")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            let sequence = sequence.checked_add(1).ok_or(Error::Conflict)?;
            sqlx::query("UPDATE team_schedule_guard SET sequence=$1 WHERE id=1")
                .bind(sequence)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            let occurrence = Uuid::new_v4();
            let run_id = Uuid::new_v4();
            let key = format!("team-schedule:{}:{}", row.value.id, occurrence);
            let due = row.value.next_run.ok_or(Error::Conflict)?;
            let request = CreateTeamRun {
                order_id: row.value.template.order_id,
                prompt: row.value.template.prompt.clone(),
                idempotency_key: key.clone(),
                plan: row.value.template.plan.clone(),
            };
            sqlx::query("INSERT INTO team_schedule_occurrences(id,schedule_id,team_id,project_id,sequence,execution_revision,grant_id,reserved_run_id,run_id,creation_key,status,due_at,claimed_at,completed_at,request,error) VALUES($1,$2,$3,$4,$5,$6,$7,$8,NULL,$9,'claimed',$10,$11,NULL,$12,NULL)").bind(occurrence.to_string()).bind(row.value.id.to_string()).bind(row.value.team_id.to_string()).bind(row.value.project_id.to_string()).bind(sequence).bind(row.value.execution_revision).bind(row.grant_id.to_string()).bind(run_id.to_string()).bind(key).bind(due).bind(stamp).bind(json_string(&request)?).execute(&mut *tx).await.map_err(storage_error)?;
            row.value.active_occurrence_id = Some(occurrence);
            row.value.updated_at = stamp;
            save(&mut tx, &row).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(TeamScheduleDispatch {
                occurrence_id: occurrence,
                schedule_id: row.value.id,
            }));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(None)
    }
    async fn note_team_schedule_admission_failure(
        &self,
        key: Uuid,
        definitely_rejected: bool,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = lock(self, &mut tx).await?;
        let occurrence = read_occurrence(&mut tx, key).await?;
        // A lost commit ACK can already have a complete linked durable run.
        if occurrence.value.run_id.is_none() {
            let mut row = read_schedule(
                &mut tx,
                occurrence.team_id,
                occurrence.project_id,
                occurrence.schedule_id,
            )
            .await?;
            row.value.paused = true;
            let actual_key_runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM team_runs WHERE team_id=$1 AND creation_key=$2 AND creator_id=(SELECT identity_id FROM team_schedule_grants WHERE id=$3)")
                .bind(occurrence.team_id.to_string()).bind(&occurrence.creation_key)
                .bind(occurrence.grant_id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            let known_absent = definitely_rejected
                && actual_key_runs == 0
                && actual_run(&mut tx, &occurrence).await?.is_none();
            let error = if known_absent {
                row.value.active_occurrence_id = None;
                "schedule_grant_or_revision_rejected"
            } else {
                "schedule_admission_uncertain"
            };
            row.value.last_error = Some(error.into());
            row.value.updated_at = stamp;
            save(&mut tx, &row).await?;
            sqlx::query("UPDATE team_schedule_occurrences SET status=$1,error=$2,completed_at=$3 WHERE id=$4 AND run_id IS NULL")
                .bind(if known_absent {"blocked"} else {"unknown"})
                .bind(error).bind(if known_absent {Some(stamp)} else {None})
                .bind(key.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }
    async fn reconcile_team_schedules(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = lock(self, &mut tx).await?;
        let rows=sqlx::query("SELECT * FROM team_schedule_occurrences WHERE status IN('claimed','queued','running') ORDER BY last_checked,sequence LIMIT 64").fetch_all(&mut *tx).await.map_err(storage_error)?;
        for item in rows {
            let occurrence = decode_occurrence(&item)?;
            sqlx::query("UPDATE team_schedule_occurrences SET last_checked=$1 WHERE id=$2")
                .bind(stamp)
                .bind(occurrence.value.id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            let mut row = read_schedule(
                &mut tx,
                occurrence.team_id,
                occurrence.project_id,
                occurrence.schedule_id,
            )
            .await?;
            if row.value.active_occurrence_id != Some(occurrence.value.id) {
                return Err(Error::Conflict);
            }
            let Some(run_id) = occurrence.value.run_id else {
                if stamp.saturating_sub(occurrence.value.claimed_at) > 300_000 {
                    row.value.paused = true;
                    row.value.last_error = Some("schedule_admission_uncertain".into());
                    row.value.updated_at = stamp;
                    save(&mut tx, &row).await?;
                    sqlx::query("UPDATE team_schedule_occurrences SET status='unknown',error='schedule_admission_uncertain' WHERE id=$1").bind(occurrence.value.id.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
                }
                continue;
            };
            let run = actual_run(&mut tx, &occurrence)
                .await?
                .ok_or(Error::Conflict)?;
            let status = serde_json::to_value(&run.status)
                .map_err(storage_error)?
                .as_str()
                .ok_or(Error::Conflict)?
                .to_owned();
            let terminal = matches!(
                run.status,
                TeamRunStatus::Completed | TeamRunStatus::Failed | TeamRunStatus::Cancelled
            );
            let unknown = run.status == TeamRunStatus::Unknown;
            sqlx::query("UPDATE team_schedule_occurrences SET status=$1,error=$2,completed_at=$3 WHERE id=$4").bind(&status).bind(run.error.as_ref().map(|_|"team_run_failed")).bind(if terminal{Some(stamp)}else{None}).bind(occurrence.value.id.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            if terminal {
                row.value.active_occurrence_id = None;
                row.value.last_run_id = Some(run_id);
                row.value.last_error = run.error.as_ref().map(|_| "team_run_failed".into());
                row.value.next_run = productivity_next_run(
                    &row.value.recurrence,
                    &row.value.timezone,
                    stamp.max(occurrence.value.due_at),
                )?;
                if row.value.next_run.is_none() {
                    row.value.paused = true;
                }
                row.value.updated_at = stamp;
                save(&mut tx, &row).await?;
            } else if unknown {
                row.value.paused = true;
                row.value.last_error = Some("team_run_unknown_requires_review".into());
                row.value.updated_at = stamp;
                save(&mut tx, &row).await?;
            }
        }
        tx.commit().await.map_err(storage_error)
    }
}

fn decode_occurrence(row: &AnyRow) -> Result<OccurrenceRow> {
    let run: Option<String> = row.try_get("run_id").map_err(storage_error)?;
    let request: CreateTeamRun =
        parse(&row.try_get::<String, _>("request").map_err(storage_error)?)?;
    let value = OccurrenceRow {
        value: TeamScheduleOccurrence {
            id: uuid(row.try_get("id").map_err(storage_error)?)?,
            sequence: row.try_get("sequence").map_err(storage_error)?,
            due_at: row.try_get("due_at").map_err(storage_error)?,
            claimed_at: row.try_get("claimed_at").map_err(storage_error)?,
            run_id: run.map(uuid).transpose()?,
            status: row.try_get("status").map_err(storage_error)?,
            error: row.try_get("error").map_err(storage_error)?,
            completed_at: row.try_get("completed_at").map_err(storage_error)?,
        },
        schedule_id: uuid(row.try_get("schedule_id").map_err(storage_error)?)?,
        team_id: uuid(row.try_get("team_id").map_err(storage_error)?)?,
        project_id: uuid(row.try_get("project_id").map_err(storage_error)?)?,
        execution_revision: row.try_get("execution_revision").map_err(storage_error)?,
        grant_id: uuid(row.try_get("grant_id").map_err(storage_error)?)?,
        reserved_run_id: uuid(row.try_get("reserved_run_id").map_err(storage_error)?)?,
        creation_key: row.try_get("creation_key").map_err(storage_error)?,
        request,
    };
    if value.request.idempotency_key != value.creation_key
        || value.creation_key != format!("team-schedule:{}:{}", value.schedule_id, value.value.id)
    {
        return Err(Error::Conflict);
    }
    Ok(value)
}
async fn read_occurrence(tx: &mut Transaction<'_, Any>, id: Uuid) -> Result<OccurrenceRow> {
    let row = sqlx::query("SELECT * FROM team_schedule_occurrences WHERE id=$1")
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    decode_occurrence(&row)
}
async fn actual_run(
    tx: &mut Transaction<'_, Any>,
    occurrence: &OccurrenceRow,
) -> Result<Option<TeamRun>> {
    let row=sqlx::query("SELECT creator_id,creation_key,request_sha,initial_receipt FROM team_runs WHERE id=$1 AND team_id=$2 AND project_id=$3 AND order_id=$4").bind(occurrence.reserved_run_id.to_string()).bind(occurrence.team_id.to_string()).bind(occurrence.project_id.to_string()).bind(occurrence.request.order_id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(row) = row else { return Ok(None) };
    let creator: String = sqlx::query_scalar(
        "SELECT identity_id FROM team_schedule_grants WHERE id=$1 AND schedule_id=$2",
    )
    .bind(occurrence.grant_id.to_string())
    .bind(occurrence.schedule_id.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if row
        .try_get::<String, _>("creator_id")
        .map_err(storage_error)?
        != creator
        || row
            .try_get::<String, _>("creation_key")
            .map_err(storage_error)?
            != occurrence.creation_key
        || row
            .try_get::<String, _>("request_sha")
            .map_err(storage_error)?
            != hash(json_string(&(occurrence.team_id, &occurrence.request))?.as_bytes())
    {
        return Err(Error::Conflict);
    }
    // Current payload is authoritative for reconciliation, initial receipt for replay.
    let payload: String = sqlx::query_scalar("SELECT payload FROM team_runs WHERE id=$1")
        .bind(occurrence.reserved_run_id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let run: TeamRun = parse(&payload)?;
    if run.id != occurrence.reserved_run_id
        || run.team_id != occurrence.team_id
        || run.project_id != occurrence.project_id
        || run.order_id != occurrence.request.order_id
    {
        return Err(Error::Conflict);
    }
    Ok(Some(run))
}
pub(crate) async fn prepare_scheduled_run_in_transaction(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    dispatch: &TeamScheduleDispatch,
) -> Result<TeamSchedulePreparation> {
    lock(store, tx).await?;
    let occurrence = read_occurrence(tx, dispatch.occurrence_id).await?;
    if occurrence.schedule_id != dispatch.schedule_id {
        return Err(Error::Forbidden);
    }
    let row = read_schedule(
        tx,
        occurrence.team_id,
        occurrence.project_id,
        occurrence.schedule_id,
    )
    .await?;
    let grant = validate_grant(tx, &row).await?;
    if occurrence.grant_id != row.grant_id
        || occurrence.execution_revision != row.value.execution_revision
        || row.value.active_occurrence_id != Some(occurrence.value.id)
    {
        return Err(Error::Forbidden);
    }
    if let Some(run_id) = occurrence.value.run_id {
        if run_id != occurrence.reserved_run_id {
            return Err(Error::Conflict);
        }
        let actual = actual_run(tx, &occurrence).await?.ok_or(Error::Conflict)?;
        let receipt: String =
            sqlx::query_scalar("SELECT initial_receipt FROM team_runs WHERE id=$1")
                .bind(actual.id.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
        return Ok(TeamSchedulePreparation::Existing(Box::new(parse(
            &receipt,
        )?)));
    }
    if row.value.paused
        || occurrence.value.status != "claimed"
        || occurrence.request.prompt != row.value.template.prompt
        || occurrence.request.order_id != row.value.template.order_id
        || json_string(&occurrence.request.plan)? != json_string(&row.value.template.plan)?
    {
        return Err(Error::Conflict);
    }
    let actor = store
        .identity()
        .issue_scheduler_session_in_transaction(
            tx,
            grant.identity_id,
            occurrence.team_id,
            occurrence.project_id,
            28_800_000,
        )
        .await
        .map_err(identity_error)?;
    sqlx::query("UPDATE team_schedule_occurrences SET run_id=$1 WHERE id=$2 AND run_id IS NULL AND status='claimed'").bind(occurrence.reserved_run_id.to_string()).bind(occurrence.value.id.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(TeamSchedulePreparation::New(Box::new(
        TeamScheduleRunPermit {
            actor,
            run_id: occurrence.reserved_run_id,
            team_id: occurrence.team_id,
            project_id: occurrence.project_id,
            project_revision: grant.project_revision,
            order_revision: grant.order_revision,
            provider_revision: grant.provider_revision,
            input: occurrence.request,
            occurrence_id: occurrence.value.id,
            schedule_id: occurrence.schedule_id,
            grant_id: occurrence.grant_id,
        },
    )))
}
pub(crate) async fn finish_scheduled_run_in_transaction(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    permit: &TeamScheduleRunPermit,
    run: &TeamRun,
) -> Result<()> {
    lock(store, tx).await?;
    if run.id != permit.run_id
        || run.team_id != permit.team_id
        || run.project_id != permit.project_id
        || run.order_id != permit.input.order_id
        || run.provider_revision != permit.provider_revision
        || run.status != TeamRunStatus::Queued
    {
        return Err(Error::Conflict);
    }
    let occurrence = read_occurrence(tx, permit.occurrence_id).await?;
    let row = read_schedule(tx, permit.team_id, permit.project_id, permit.schedule_id).await?;
    let grant = validate_grant(tx, &row).await?;
    if grant.identity_id != permit.actor.identity_id()
        || occurrence.value.run_id != Some(permit.run_id)
        || occurrence.reserved_run_id != permit.run_id
        || occurrence.grant_id != permit.grant_id
        || row.grant_id != permit.grant_id
        || row.value.active_occurrence_id != Some(permit.occurrence_id)
        || row.value.paused
        || occurrence.value.status != "claimed"
    {
        return Err(Error::Conflict);
    }
    actual_run(tx, &occurrence).await?.ok_or(Error::Conflict)?;
    sqlx::query("UPDATE team_schedule_occurrences SET status='queued',error=NULL WHERE id=$1 AND run_id=$2 AND status='claimed'").bind(permit.occurrence_id.to_string()).bind(permit.run_id.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn validate_scheduled_run_in_transaction(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    run_id: Uuid,
) -> Result<()> {
    // Normal interactive runs have no scheduler grant and retain their original authority.
    let row = sqlx::query(
        "SELECT * FROM team_schedule_occurrences WHERE run_id=$1 OR reserved_run_id=$1",
    )
    .bind(run_id.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else { return Ok(()) };
    lock(store, tx).await?;
    let occurrence = decode_occurrence(&row)?;
    let schedule = read_schedule(
        tx,
        occurrence.team_id,
        occurrence.project_id,
        occurrence.schedule_id,
    )
    .await?;
    let grant = validate_grant(tx, &schedule).await?;
    if occurrence.value.run_id != Some(run_id)
        || occurrence.reserved_run_id != run_id
        || occurrence.grant_id != schedule.grant_id
        || occurrence.execution_revision != schedule.value.execution_revision
        || schedule.value.active_occurrence_id != Some(occurrence.value.id)
        || !matches!(
            occurrence.value.status.as_str(),
            "claimed" | "queued" | "running"
        )
    {
        return Err(Error::Forbidden);
    }
    if occurrence.value.status == "claimed" && actual_run(tx, &occurrence).await?.is_some() {
        return Err(Error::Conflict);
    }
    if occurrence.value.status != "claimed"
        && actual_run(tx, &occurrence)
            .await?
            .ok_or(Error::Forbidden)?
            .provider_revision
            != grant.provider_revision
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
