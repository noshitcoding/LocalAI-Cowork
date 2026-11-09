//! Agent access to the two knowledge bases: the project knowledge base and the
//! personal (per user) knowledge base. These switches only narrow which already
//! readable entries may enter a new run's context; they never grant read, file,
//! tool or sharing authority, and they never change a run that was already admitted.
//!
//! Local owner installations keep their own preferences. On a team server only a
//! platform administrator may enable access, globally or as explicit exceptions:
//! per user for that user's private projects, per created team project, and per
//! user for adding their personal knowledge base to team runs they start. The
//! last switch is separate because private content then reaches team results.
use super::*;
use cowork_identity::Principal;
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction};
use std::collections::BTreeSet;

const OWNER_SCOPE: &str = "owner";
const POLICY_ID: &str = "policy";
const MAX_EXCEPTIONS: usize = 1000;
const DIRECTORY_LIMIT: usize = 500;
/// Bounds for the personal knowledge frozen into one team run.
const PERSONAL_TEAM_ENTRIES: usize = 20;
const PERSONAL_TEAM_CHARS: usize = 4000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerKnowledgeAccess {
    /// 0 while the defaults have never been changed.
    pub revision: i64,
    pub personal_knowledge: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriteOwnerKnowledgeAccess {
    pub expected_revision: i64,
    pub personal_knowledge: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectKnowledgeAccess {
    pub project_id: String,
    /// 0 while the project still uses the defaults.
    pub revision: i64,
    pub project_knowledge: bool,
    pub personal_knowledge: bool,
    /// The owner-wide switch for the personal knowledge base.
    pub personal_available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriteProjectKnowledgeAccess {
    pub expected_revision: i64,
    pub project_knowledge: bool,
    pub personal_knowledge: bool,
}
/// The effective local decision for one project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalKnowledgeGate {
    pub project: bool,
    pub personal: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeAccessPolicy {
    /// 0 while the server still uses the default: everything disabled.
    pub revision: i64,
    /// Global switch for every user's private projects.
    pub private_projects: bool,
    /// Global switch for every created team project.
    pub team_projects: bool,
    /// Users whose private projects are enabled although the global switch is off.
    pub enabled_users: Vec<Uuid>,
    /// Team projects enabled although the global switch is off.
    pub enabled_projects: Vec<Uuid>,
    pub updated_at: Option<i64>,
    pub updated_by: Option<Uuid>,
    /// Global switch: every user may add their personal knowledge base to team runs
    /// they start. Missing in policies stored before this switch existed.
    #[serde(default)]
    pub personal_team_runs: bool,
    /// Users who may do so although the global switch is off.
    #[serde(default)]
    pub enabled_personal_team_users: Vec<Uuid>,
}
impl KnowledgeAccessPolicy {
    pub fn private_allowed(&self, identity: Uuid) -> bool {
        self.private_projects || self.enabled_users.contains(&identity)
    }
    pub fn team_allowed(&self, project: Uuid) -> bool {
        self.team_projects || self.enabled_projects.contains(&project)
    }
    pub fn personal_team_allowed(&self, identity: Uuid) -> bool {
        self.personal_team_runs || self.enabled_personal_team_users.contains(&identity)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriteKnowledgeAccessPolicy {
    pub expected_revision: i64,
    pub private_projects: bool,
    pub team_projects: bool,
    pub enabled_users: Vec<Uuid>,
    pub enabled_projects: Vec<Uuid>,
    /// None keeps the stored value, so clients that predate the switch cannot
    /// silently clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_team_runs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_personal_team_users: Option<Vec<Uuid>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeAccessUser {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub active: bool,
    pub private_projects: bool,
    pub personal_team_runs: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeAccessProject {
    pub id: Uuid,
    pub name: String,
    pub team_id: Uuid,
    pub team_name: String,
    pub archived: bool,
    pub team_projects: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeAccessDirectory {
    pub policy: KnowledgeAccessPolicy,
    pub users: Vec<KnowledgeAccessUser>,
    pub projects: Vec<KnowledgeAccessProject>,
    pub truncated: bool,
}
/// What a signed-in server user may know about their own effective access.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActorKnowledgeAccess {
    pub private_projects: bool,
    /// May add the personal knowledge base to team runs this user starts.
    pub personal_team_runs: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TeamProjectKnowledgeAccess {
    pub project_id: Uuid,
    pub project_knowledge: bool,
}
/// One account-wide memory of the person who started a team run, frozen at
/// admission. It is private content that may appear in results the team reads.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TeamPersonalKnowledge {
    pub id: Uuid,
    pub revision: i64,
    pub key: String,
    pub category: String,
    pub content: String,
}

pub(crate) async fn migrate(tx: &mut Transaction<'_, Any>) -> Result<()> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS knowledge_access_schema_guard(version BIGINT PRIMARY KEY CHECK(version=1))",
        "CREATE TABLE IF NOT EXISTS native_knowledge_access(scope TEXT PRIMARY KEY, revision BIGINT NOT NULL CHECK(revision>0), project_knowledge BIGINT NOT NULL CHECK(project_knowledge IN(0,1)), personal_knowledge BIGINT NOT NULL CHECK(personal_knowledge IN(0,1)), updated_at BIGINT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS server_knowledge_access(id TEXT PRIMARY KEY CHECK(id='policy'), revision BIGINT NOT NULL CHECK(revision>0), payload TEXT NOT NULL)",
    ] {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    let rows = sqlx::query("SELECT version FROM knowledge_access_schema_guard")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
    if rows.is_empty() {
        sqlx::query("INSERT INTO knowledge_access_schema_guard(version) VALUES(1)")
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    } else if rows.len() != 1
        || rows[0]
            .try_get::<i64, _>("version")
            .map_err(storage_error)?
            != 1
    {
        return Err(Error::Validation(
            "Unsupported knowledge access schema".into(),
        ));
    }
    Ok(())
}

fn flag(value: bool) -> i64 {
    i64::from(value)
}
fn stored_flag(row: &sqlx::any::AnyRow, name: &str) -> Result<bool> {
    match row.try_get::<i64, _>(name).map_err(storage_error)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Validation("Invalid knowledge access record".into())),
    }
}
/// (revision, project_knowledge, personal_knowledge); defaults keep the existing
/// single-owner behaviour where both knowledge bases are available.
async fn local_row(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<(i64, bool, bool)> {
    let row = sqlx::query("SELECT revision,project_knowledge,personal_knowledge FROM native_knowledge_access WHERE scope=$1")
        .bind(scope)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok((0, true, true));
    };
    Ok((
        row.try_get::<i64, _>("revision").map_err(storage_error)?,
        stored_flag(&row, "project_knowledge")?,
        stored_flag(&row, "personal_knowledge")?,
    ))
}
async fn write_local_row(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    expected: i64,
    project: bool,
    personal: bool,
) -> Result<i64> {
    let current = local_row(tx, scope).await?;
    if current.0 == expected + 1 && current.1 == project && current.2 == personal {
        // An exact retry after a lost acknowledgement observes its own result.
        return Ok(current.0);
    }
    if current.0 != expected {
        return Err(Error::Conflict);
    }
    let changed = if expected == 0 {
        sqlx::query("INSERT INTO native_knowledge_access(scope,revision,project_knowledge,personal_knowledge,updated_at) VALUES($1,1,$2,$3,$4) ON CONFLICT(scope) DO NOTHING")
            .bind(scope).bind(flag(project)).bind(flag(personal)).bind(now())
            .execute(&mut **tx).await.map_err(storage_error)?
    } else {
        sqlx::query("UPDATE native_knowledge_access SET revision=revision+1,project_knowledge=$2,personal_knowledge=$3,updated_at=$4 WHERE scope=$1 AND revision=$5")
            .bind(scope).bind(flag(project)).bind(flag(personal)).bind(now()).bind(expected)
            .execute(&mut **tx).await.map_err(storage_error)?
    };
    if changed.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    Ok(expected + 1)
}
async fn local_project_exists(tx: &mut Transaction<'_, Any>, project: &str) -> Result<()> {
    valid_text(project, 200)?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id=$1 AND id NOT IN (SELECT project_id FROM native_project_retirements)")
        .bind(project)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    if count != 1 {
        return Err(Error::NotFound);
    }
    Ok(())
}
async fn project_view(
    tx: &mut Transaction<'_, Any>,
    project: &str,
) -> Result<ProjectKnowledgeAccess> {
    let owner = local_row(tx, OWNER_SCOPE).await?;
    let (revision, project_knowledge, personal_knowledge) =
        local_row(tx, &format!("project:{project}")).await?;
    Ok(ProjectKnowledgeAccess {
        project_id: project.into(),
        revision,
        project_knowledge,
        personal_knowledge,
        personal_available: owner.2,
    })
}
/// Used inside the run-admission transaction, so the decision is frozen with the context.
pub(crate) async fn local_gate(
    tx: &mut Transaction<'_, Any>,
    project: &str,
) -> Result<LocalKnowledgeGate> {
    let value = project_view(tx, project).await?;
    Ok(LocalKnowledgeGate {
        project: value.project_knowledge,
        personal: value.personal_available && value.personal_knowledge,
    })
}

async fn policy(tx: &mut Transaction<'_, Any>) -> Result<KnowledgeAccessPolicy> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT payload FROM server_knowledge_access WHERE id=$1")
            .bind(POLICY_ID)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    raw.map_or_else(|| Ok(KnowledgeAccessPolicy::default()), |raw| parse(&raw))
}
pub(crate) async fn private_allowed(tx: &mut Transaction<'_, Any>, identity: Uuid) -> Result<bool> {
    Ok(policy(tx).await?.private_allowed(identity))
}
pub(crate) async fn team_allowed(tx: &mut Transaction<'_, Any>, project: Uuid) -> Result<bool> {
    Ok(policy(tx).await?.team_allowed(project))
}
pub(crate) async fn personal_team_allowed(
    tx: &mut Transaction<'_, Any>,
    identity: Uuid,
) -> Result<bool> {
    Ok(policy(tx).await?.personal_team_allowed(identity))
}
/// The enabled account-wide memories (personal knowledge base) of `identity` for a
/// team run, read inside the admission transaction so they are frozen with it.
/// Entries matching the prompt come first; the result is bounded and truncated.
pub(crate) async fn personal_team_knowledge(
    tx: &mut Transaction<'_, Any>,
    identity: Uuid,
    prompt: &str,
) -> Result<Vec<TeamPersonalKnowledge>> {
    let rows = sqlx::query("SELECT entity_id,revision,payload FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type='memory' AND team_id IS NULL AND project_id IS NULL AND tombstone=0 ORDER BY entity_id LIMIT 1001")
        .bind(identity.to_string())
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
    if rows.len() > 1000 {
        return Err(Error::BudgetExceeded);
    }
    let words: BTreeSet<_> = prompt
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_lowercase)
        .take(64)
        .collect();
    let mut entries = Vec::new();
    for row in rows {
        let stored: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let value = &stored["payload"];
        let (Some(key), Some(content)) = (value["key"].as_str(), value["content"].as_str()) else {
            continue;
        };
        if value["enabled"] == false || key.trim().is_empty() || content.trim().is_empty() {
            continue;
        }
        let category = value["category"].as_str().unwrap_or_default();
        let lower = format!("{key} {category} {content}").to_lowercase();
        let score = words.iter().filter(|w| lower.contains(w.as_str())).count();
        entries.push((
            score,
            TeamPersonalKnowledge {
                id: Uuid::parse_str(
                    &row.try_get::<String, _>("entity_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                revision: row.try_get("revision").map_err(storage_error)?,
                key: key.chars().take(200).collect(),
                category: category.chars().take(80).collect(),
                content: content.chars().take(PERSONAL_TEAM_CHARS).collect(),
            },
        ));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    Ok(entries
        .into_iter()
        .take(PERSONAL_TEAM_ENTRIES)
        .map(|(_, entry)| entry)
        .collect())
}
/// How frozen personal knowledge appears in a team run's model context: marked as
/// the run creator's personal knowledge, apart from team skills and knowledge.
pub fn team_personal_context(entries: &[TeamPersonalKnowledge]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut context = String::from(
        "\nEntries marked untrusted_personal_context are personal knowledge of the person who started this run, not team knowledge. Results are visible to the team; use them only where the task needs them.\n",
    );
    for entry in entries {
        context.push_str(&format!(
            "\n<untrusted_personal_context id=\"{}\" revision=\"{}\" key={:?} category={:?}>\n{}\n</untrusted_personal_context>\n",
            entry.id, entry.revision, entry.key, entry.category, entry.content
        ));
    }
    context
}
fn exceptions(values: &[Uuid]) -> Result<Vec<Uuid>> {
    let unique: BTreeSet<_> = values.iter().copied().collect();
    if unique.len() != values.len() || values.len() > MAX_EXCEPTIONS {
        return Err(Error::Validation(
            "Invalid knowledge access exceptions".into(),
        ));
    }
    Ok(unique.into_iter().collect())
}
async fn platform_admin(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
) -> Result<()> {
    store
        .identity()
        .require_global_session_in_transaction(tx, actor)
        .await
        .map_err(team_service::identity_error)?;
    let identity = store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(team_service::identity_error)?;
    if !identity.platform_admin {
        return Err(Error::Forbidden);
    }
    Ok(())
}

impl Store {
    pub async fn owner_knowledge_access(&self) -> Result<OwnerKnowledgeAccess> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (revision, _, personal_knowledge) = local_row(&mut tx, OWNER_SCOPE).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(OwnerKnowledgeAccess {
            revision,
            personal_knowledge,
        })
    }
    pub async fn write_owner_knowledge_access(
        &self,
        input: &WriteOwnerKnowledgeAccess,
    ) -> Result<OwnerKnowledgeAccess> {
        if input.expected_revision < 0 {
            return Err(Error::Validation(
                "Invalid knowledge access revision".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let revision = write_local_row(
            &mut tx,
            OWNER_SCOPE,
            input.expected_revision,
            true,
            input.personal_knowledge,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(OwnerKnowledgeAccess {
            revision,
            personal_knowledge: input.personal_knowledge,
        })
    }
    pub async fn project_knowledge_access(&self, project: &str) -> Result<ProjectKnowledgeAccess> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        local_project_exists(&mut tx, project).await?;
        let value = project_view(&mut tx, project).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn write_project_knowledge_access(
        &self,
        project: &str,
        input: &WriteProjectKnowledgeAccess,
    ) -> Result<ProjectKnowledgeAccess> {
        if input.expected_revision < 0 {
            return Err(Error::Validation(
                "Invalid knowledge access revision".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        local_project_exists(&mut tx, project).await?;
        write_local_row(
            &mut tx,
            &format!("project:{project}"),
            input.expected_revision,
            input.project_knowledge,
            input.personal_knowledge,
        )
        .await?;
        let value = project_view(&mut tx, project).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }

    /// Platform administrators only. Lists bounded user and team-project
    /// directories so exceptions can be chosen; it reveals no content.
    pub async fn knowledge_access_directory(
        &self,
        actor: &Principal,
    ) -> Result<KnowledgeAccessDirectory> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        platform_admin(self, &mut tx, actor).await?;
        let policy = policy(&mut tx).await?;
        let rows = sqlx::query("SELECT id,username,display_name,active FROM identity_subjects ORDER BY username,id LIMIT 501")
            .fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut truncated = rows.len() > DIRECTORY_LIMIT;
        let mut users = Vec::new();
        for row in rows.iter().take(DIRECTORY_LIMIT) {
            let id = Uuid::parse_str(&row.try_get::<String, _>("id").map_err(storage_error)?)
                .map_err(storage_error)?;
            users.push(KnowledgeAccessUser {
                id,
                username: row.try_get("username").map_err(storage_error)?,
                display_name: row.try_get("display_name").map_err(storage_error)?,
                active: row.try_get::<i64, _>("active").map_err(storage_error)? == 1,
                private_projects: policy.private_allowed(id),
                personal_team_runs: policy.personal_team_allowed(id),
            });
        }
        let rows = sqlx::query("SELECT p.id,p.organization_id,p.payload,o.name AS team_name,pr.name AS profile_name,pr.archived AS archived FROM tenant_projects p JOIN identity_organizations o ON o.id=p.organization_id LEFT JOIN team_project_profiles pr ON pr.project_id=p.id ORDER BY o.name,p.id LIMIT 501")
            .fetch_all(&mut *tx).await.map_err(storage_error)?;
        truncated |= rows.len() > DIRECTORY_LIMIT;
        let mut projects = Vec::new();
        for row in rows.iter().take(DIRECTORY_LIMIT) {
            let id = Uuid::parse_str(&row.try_get::<String, _>("id").map_err(storage_error)?)
                .map_err(storage_error)?;
            let stored: Value =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let name = row
                .try_get::<Option<String>, _>("profile_name")
                .map_err(storage_error)?
                .or_else(|| stored["name"].as_str().map(str::to_owned))
                .unwrap_or_default();
            projects.push(KnowledgeAccessProject {
                id,
                name,
                team_id: Uuid::parse_str(
                    &row.try_get::<String, _>("organization_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                team_name: row.try_get("team_name").map_err(storage_error)?,
                archived: row
                    .try_get::<Option<i64>, _>("archived")
                    .map_err(storage_error)?
                    == Some(1),
                team_projects: policy.team_allowed(id),
            });
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(KnowledgeAccessDirectory {
            policy,
            users,
            projects,
            truncated,
        })
    }
    pub async fn write_knowledge_access_policy(
        &self,
        actor: &Principal,
        input: &WriteKnowledgeAccessPolicy,
    ) -> Result<KnowledgeAccessPolicy> {
        if input.expected_revision < 0 {
            return Err(Error::Validation(
                "Invalid knowledge access revision".into(),
            ));
        }
        let enabled_users = exceptions(&input.enabled_users)?;
        let enabled_projects = exceptions(&input.enabled_projects)?;
        let personal_team_users = input
            .enabled_personal_team_users
            .as_deref()
            .map(exceptions)
            .transpose()?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        platform_admin(self, &mut tx, actor).await?;
        let current = policy(&mut tx).await?;
        let personal_team_runs = input
            .personal_team_runs
            .unwrap_or(current.personal_team_runs);
        let enabled_personal_team_users =
            personal_team_users.unwrap_or_else(|| current.enabled_personal_team_users.clone());
        if current.revision == input.expected_revision + 1
            && current.private_projects == input.private_projects
            && current.team_projects == input.team_projects
            && current.enabled_users == enabled_users
            && current.enabled_projects == enabled_projects
            && current.personal_team_runs == personal_team_runs
            && current.enabled_personal_team_users == enabled_personal_team_users
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(current);
        }
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        for id in enabled_users.iter().chain(&enabled_personal_team_users) {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects WHERE id=$1")
                    .bind(id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            if count != 1 {
                return Err(Error::Validation(
                    "Unknown user in knowledge access exceptions".into(),
                ));
            }
        }
        for id in &enabled_projects {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenant_projects WHERE id=$1")
                .bind(id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            if count != 1 {
                return Err(Error::Validation(
                    "Unknown project in knowledge access exceptions".into(),
                ));
            }
        }
        let next = KnowledgeAccessPolicy {
            revision: input.expected_revision + 1,
            private_projects: input.private_projects,
            team_projects: input.team_projects,
            enabled_users,
            enabled_projects,
            updated_at: Some(now()),
            updated_by: Some(actor.identity_id()),
            personal_team_runs,
            enabled_personal_team_users,
        };
        let changed = if input.expected_revision == 0 {
            sqlx::query("INSERT INTO server_knowledge_access(id,revision,payload) VALUES($1,1,$2) ON CONFLICT(id) DO NOTHING")
                .bind(POLICY_ID).bind(json_string(&next)?)
                .execute(&mut *tx).await.map_err(storage_error)?
        } else {
            sqlx::query("UPDATE server_knowledge_access SET revision=$1,payload=$2 WHERE id=$3 AND revision=$4")
                .bind(next.revision).bind(json_string(&next)?).bind(POLICY_ID).bind(input.expected_revision)
                .execute(&mut *tx).await.map_err(storage_error)?
        };
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(next)
    }
    pub async fn actor_knowledge_access(&self, actor: &Principal) -> Result<ActorKnowledgeAccess> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        let policy = policy(&mut tx).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(ActorKnowledgeAccess {
            private_projects: policy.private_allowed(actor.identity_id()),
            personal_team_runs: policy.personal_team_allowed(actor.identity_id()),
        })
    }
    /// The caller must already have authorized the actor for this team project.
    pub async fn team_project_knowledge_access(
        &self,
        project: Uuid,
    ) -> Result<TeamProjectKnowledgeAccess> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let project_knowledge = team_allowed(&mut tx, project).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamProjectKnowledgeAccess {
            project_id: project,
            project_knowledge,
        })
    }
}
