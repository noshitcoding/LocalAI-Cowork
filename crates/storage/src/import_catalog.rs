//! Typed legacy definitions. Only fresh current authority may bind them to V2.
use super::*;
use cowork_contracts::{TeamDocument, TeamDocumentKind, TeamWorkflowPlan};
use cowork_identity::{MigrationIdentityBinding, OrganizationRole};

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS v1_migration_configurations(id TEXT PRIMARY KEY,scope TEXT NOT NULL CHECK(scope IN('personal','team')),team_id TEXT,project_id TEXT NOT NULL,receipt_id TEXT NOT NULL,source_id TEXT NOT NULL,source_kind TEXT NOT NULL,payload TEXT NOT NULL,UNIQUE(scope,receipt_id,source_kind,source_id))",
    "CREATE INDEX IF NOT EXISTS v1_migration_configuration_project ON v1_migration_configurations(scope,team_id,project_id)",
];
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1SourceUser {
    pub source_id: String,
    pub username: String,
    pub display_name: String,
    pub active: bool,
    pub source_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1SourceOrganization {
    pub source_id: String,
    pub name: String,
    pub owner_source_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1SourceMembership {
    pub source_organization_id: String,
    pub source_user_id: String,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1SourceAcl {
    pub source_project_id: String,
    pub source_user_id: String,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1DefinitionVersion {
    pub source_id: String,
    pub revision: i64,
    pub content: String,
    pub summary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1MigrationObject {
    pub kind: String,
    pub source_id: String,
    pub source_project_id: Option<String>,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub source_revision: i64,
    pub versions: Vec<V1DefinitionVersion>,
    pub workflow: Option<TeamWorkflowPlan>,
    pub schedule_expression: Option<String>,
    pub timezone: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub paused: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1OperationalState {
    pub source_id: String,
    pub source_kind: String,
    pub state: String,
    pub revision: i64,
    pub execution_blocked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1MigrationCatalog {
    pub version: u32,
    pub users: Vec<V1SourceUser>,
    pub organizations: Vec<V1SourceOrganization>,
    pub memberships: Vec<V1SourceMembership>,
    pub project_acl: Vec<V1SourceAcl>,
    pub objects: Vec<V1MigrationObject>,
    pub operational_states: Vec<V1OperationalState>,
    pub limitations: Vec<String>,
}
impl Default for V1MigrationCatalog {
    fn default() -> Self {
        Self {
            version: 1,
            users: vec![],
            organizations: vec![],
            memberships: vec![],
            project_acl: vec![],
            objects: vec![],
            operational_states: vec![],
            limitations: vec![],
        }
    }
}
impl V1MigrationCatalog {
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
            && self.organizations.is_empty()
            && self.memberships.is_empty()
            && self.project_acl.is_empty()
            && self.objects.is_empty()
            && self.operational_states.is_empty()
            && self.limitations.is_empty()
    }
    pub(super) fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.users.len() > 1000
            || self.organizations.len() > 1000
            || self.memberships.len() > 4000
            || self.project_acl.len() > 4000
            || self.objects.len() > 10000
            || self.operational_states.len() > 10000
            || self.limitations.len() > 10000
            || json_string(self)?.len() > 16 * 1024 * 1024
        {
            return Err(Error::Validation("Oversized migration catalog".into()));
        }
        let mut users = std::collections::BTreeSet::new();
        let mut orgs = std::collections::BTreeSet::new();
        let mut objects = std::collections::BTreeSet::new();
        let mut members = std::collections::BTreeSet::new();
        for u in &self.users {
            valid_text(&u.source_id, 256)?;
            valid_text(&u.username, 200)?;
            valid_text(&u.display_name, 200)?;
            if u.source_revision < 1 || !users.insert(&u.source_id) {
                return Err(Error::Conflict);
            }
        }
        for o in &self.organizations {
            valid_text(&o.name, 200)?;
            valid_text(&o.source_id, 256)?;
            if !orgs.insert(&o.source_id) || !users.contains(&o.owner_source_id) {
                return Err(Error::Conflict);
            }
        }
        for m in &self.memberships {
            if !users.contains(&m.source_user_id)
                || !orgs.contains(&m.source_organization_id)
                || !members.insert((&m.source_organization_id, &m.source_user_id))
                || !matches!(m.role.as_str(), "owner" | "admin" | "member" | "viewer")
            {
                return Err(Error::Conflict);
            }
        }
        for a in &self.project_acl {
            valid_text(&a.source_project_id, 256)?;
            if !users.contains(&a.source_user_id)
                || !matches!(a.role.as_str(), "viewer" | "runner" | "editor" | "owner")
            {
                return Err(Error::Conflict);
            }
        }
        for o in &self.objects {
            valid_text(&o.source_id, 256)?;
            valid_text(&o.name, 200)?;
            if !matches!(
                o.kind.as_str(),
                "skill" | "persona" | "knowledge" | "memory" | "provider" | "workflow" | "schedule"
            ) || !objects.insert((&o.kind, &o.source_id))
                || o.source_revision < 1
                || !o.paused
                || o.instructions.len() > 256000
                || o.instructions.contains('\0')
                || o.description.len() > 4000
                || o.versions.len() > 1000
            {
                return Err(Error::Validation("Invalid migration definition".into()));
            }
            let mut versions = std::collections::BTreeSet::new();
            for v in &o.versions {
                valid_text(&v.source_id, 300)?;
                if !versions.insert(&v.source_id)
                    || v.revision < 1
                    || v.content.len() > 256000
                    || v.content.contains('\0')
                    || v.summary.len() > 4000
                {
                    return Err(Error::Conflict);
                }
            }
            for v in [
                &o.source_project_id,
                &o.schedule_expression,
                &o.timezone,
                &o.model,
            ]
            .into_iter()
            .flatten()
            {
                if !v.is_empty() {
                    valid_text(v, 256)?;
                }
            }
            if let Some(v) = &o.base_url
                && !v.is_empty()
            {
                let u = reqwest::Url::parse(v)
                    .map_err(|_| Error::Validation("Invalid imported provider URL".into()))?;
                if !matches!(u.scheme(), "http" | "https")
                    || !u.username().is_empty()
                    || u.password().is_some()
                    || u.query().is_some()
                    || u.fragment().is_some()
                {
                    return Err(Error::Validation(
                        "Imported provider URL must not contain credentials".into(),
                    ));
                }
            }
            if let Some(w) = &o.workflow {
                validate_workflow(w)?;
            }
        }
        for s in &self.operational_states {
            valid_text(&s.source_id, 300)?;
            valid_text(&s.source_kind, 64)?;
            valid_text(&s.state, 120)?;
            if !s.execution_blocked || s.revision < 1 {
                return Err(Error::Conflict);
            }
        }
        for l in &self.limitations {
            valid_text(l, 4000)?;
        }
        Ok(())
    }
}
fn validate_workflow(w: &TeamWorkflowPlan) -> Result<()> {
    if w.agents.is_empty() || w.agents.len() > 16 || w.steps.is_empty() || w.steps.len() > 32 {
        return Err(Error::Conflict);
    }
    let mut agents = std::collections::BTreeSet::new();
    let mut steps = std::collections::BTreeSet::new();
    for a in &w.agents {
        valid_text(&a.id, 64)?;
        if a.instructions.len() > 64000
            || !a.skills.is_empty()
            || !a.knowledge.is_empty()
            || !agents.insert(&a.id)
        {
            return Err(Error::Conflict);
        }
    }
    for s in &w.steps {
        valid_text(&s.id, 64)?;
        valid_text(&s.prompt, 64000)?;
        if !agents.contains(&s.agent_id) || !steps.insert(&s.id) {
            return Err(Error::Conflict);
        }
    }
    let mut done = std::collections::BTreeSet::new();
    while done.len() < steps.len() {
        let before = done.len();
        for s in &w.steps {
            if s.depends_on
                .iter()
                .any(|d| !steps.contains(d) || d == &s.id)
            {
                return Err(Error::Conflict);
            }
            if s.depends_on.iter().all(|d| done.contains(d)) {
                done.insert(&s.id);
            }
        }
        if before == done.len() {
            return Err(Error::Conflict);
        }
    }
    if matches!(w.mode, cowork_contracts::TeamWorkflowMode::Hierarchical)
        && w.manager_agent_id
            .as_ref()
            .is_none_or(|id| !agents.contains(id))
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum V1IdentityTargetPlan {
    Archive,
    Existing {
        identity_id: Uuid,
        expected_revision: i64,
    },
    New {
        username: String,
        display_name: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1GrantPlan {
    pub project_id: Uuid,
    pub expected_resource_revision: i64,
    pub expected_grant_revision: i64,
    pub write: bool,
    pub share: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1UserMapping {
    pub source_user_id: String,
    pub target: V1IdentityTargetPlan,
    pub role: OrganizationRole,
    pub expected_membership_revision: i64,
    pub project_grants: Vec<V1GrantPlan>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1ProjectDestination {
    pub source_project_id: String,
    pub project_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1ProviderAuthorization {
    pub source_id: String,
    pub base_url: String,
    pub model: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1MigrationMapping {
    #[serde(default)]
    pub users: Vec<V1UserMapping>,
    #[serde(default)]
    pub project_destinations: Vec<V1ProjectDestination>,
    #[serde(default)]
    pub selected_object_ids: Vec<String>,
    #[serde(default)]
    pub object_selection_complete: bool,
    #[serde(default)]
    pub provider: Option<V1ProviderAuthorization>,
}
impl V1MigrationMapping {
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
            && self.project_destinations.is_empty()
            && self.selected_object_ids.is_empty()
            && self.provider.is_none()
            && !self.object_selection_complete
    }
}
// Secrets are a separate commit-only type: no Serialize or Debug implementation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1FreshPassword {
    pub source_user_id: String,
    pub password: String,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1MigrationSecrets {
    #[serde(default)]
    pub passwords: Vec<V1FreshPassword>,
    #[serde(default)]
    pub provider_api_key: Option<String>,
}
impl Drop for V1MigrationSecrets {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for p in &mut self.passwords {
            p.password.zeroize();
        }
        if let Some(p) = &mut self.provider_api_key {
            p.zeroize();
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1ProjectPin {
    pub project_id: Uuid,
    pub project_revision: i64,
    pub profile_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1ImportedConfiguration {
    pub id: String,
    pub project_id: String,
    pub source: V1MigrationObject,
    pub document_id: Option<String>,
    pub state: String,
    pub revision: i64,
}
pub(super) fn configurations(
    catalog: &V1MigrationCatalog,
    destinations: &[(String, String)],
    fallback: &str,
    selected: &[String],
) -> Result<Vec<V1ImportedConfiguration>> {
    catalog.validate()?;
    let mut result = vec![];
    for o in &catalog.objects {
        let key = format!("{}:{}", o.kind, o.source_id);
        if !selected.is_empty() && !selected.contains(&key) {
            continue;
        }
        let project = o
            .source_project_id
            .as_ref()
            .and_then(|s| destinations.iter().find(|(id, _)| id == s))
            .map(|(_, p)| p.as_str())
            .unwrap_or(fallback);
        result.push(V1ImportedConfiguration {
            id: id(),
            project_id: project.into(),
            source: o.clone(),
            document_id: None,
            state: if matches!(o.kind.as_str(), "schedule" | "workflow") {
                "paused"
            } else if o.kind == "provider" {
                "requires_authorization"
            } else {
                "ready"
            }
            .into(),
            revision: 1,
        });
    }
    Ok(result)
}
pub(super) async fn insert_configurations(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    scope: &str,
    team: Option<Uuid>,
    receipt: &str,
    items: &mut [V1ImportedConfiguration],
) -> Result<()> {
    for c in items {
        let o = &c.source;
        if matches!(
            o.kind.as_str(),
            "skill" | "persona" | "knowledge" | "memory"
        ) && !o.instructions.is_empty()
        {
            if let Some(team) = team {
                let kind = if matches!(o.kind.as_str(), "skill" | "persona") {
                    TeamDocumentKind::Skill
                } else {
                    TeamDocumentKind::Knowledge
                };
                let value = TeamDocument {
                    provenance: crate::knowledge_provenance::imported_source(
                        o,
                        o.source_revision,
                        &o.source_id,
                    ),
                    id: Uuid::new_v4(),
                    team_id: team,
                    project_id: Uuid::parse_str(&c.project_id).map_err(storage_error)?,
                    kind,
                    name: o.name.clone(),
                    content: o.instructions.clone(),
                    revision: 1,
                    archived: false,
                };
                if value.content.len() > 128 * 1024 {
                    return Err(Error::Validation("Team document exceeds 128 KiB".into()));
                }
                sqlx::query(
                    "INSERT INTO team_documents(id,team_id,project_id,payload) VALUES($1,$2,$3,$4)",
                )
                .bind(value.id.to_string())
                .bind(team.to_string())
                .bind(&c.project_id)
                .bind(json_string(&value)?)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
                crate::knowledge_provenance::record(
                    tx,
                    &crate::knowledge_provenance::scope(&c.project_id, Some(team)),
                    &value.id.to_string(),
                    value.revision,
                    &value.content,
                    &value.provenance,
                    &value,
                )
                .await?;
                c.document_id = Some(value.id.to_string());
            } else if matches!(o.kind.as_str(), "skill" | "persona") {
                let doc = crate::productivity::write_skill_in_tx(
                    tx,
                    &c.project_id,
                    WriteSkill {
                        run_mode: Default::default(),
                        auto_learn: false,
                        id: None,
                        expected_revision: 0,
                        name: o.name.clone(),
                        description: o.description.clone(),
                        instructions: o.instructions.clone(),
                        trigger_pattern: None,
                        prompt_template: None,
                        enabled: true,
                    },
                )
                .await?;
                c.document_id = Some(doc.id);
            } else {
                let doc = crate::productivity::write_knowledge_in_tx(
                    tx,
                    &c.project_id,
                    WriteKnowledge {
                        location: None,
                        memory: None,
                        provenance: Some(crate::knowledge_provenance::imported_source(
                            o,
                            o.source_revision,
                            &o.source_id,
                        )),
                        id: None,
                        expected_revision: 0,
                        kind: if o.kind == "memory" {
                            KnowledgeKind::Memory
                        } else {
                            KnowledgeKind::Knowledge
                        },
                        title: o.name.clone(),
                        content: o.instructions.clone(),
                        tags: vec!["v1-import".into()],
                    },
                )
                .await?;
                c.document_id = Some(doc.id);
            }
        }
        sqlx::query("INSERT INTO v1_migration_configurations(id,scope,team_id,project_id,receipt_id,source_id,source_kind,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(&c.id).bind(scope).bind(team.map(|id|id.to_string())).bind(&c.project_id).bind(receipt).bind(&o.source_id).bind(&o.kind).bind(json_string(c)?).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(())
}
pub(super) fn identity_inputs(
    mapping: &V1MigrationMapping,
    secrets: &mut V1MigrationSecrets,
) -> Result<Vec<cowork_identity::MigrationIdentityMapping>> {
    let mut result = vec![];
    let mut used = std::collections::BTreeSet::new();
    for m in &mapping.users {
        let target = match &m.target {
            V1IdentityTargetPlan::Archive => {
                if !m.project_grants.is_empty() {
                    return Err(Error::Conflict);
                }
                continue;
            }
            V1IdentityTargetPlan::Existing {
                identity_id,
                expected_revision,
            } => cowork_identity::MigrationIdentityTarget::Existing {
                identity_id: *identity_id,
                expected_revision: *expected_revision,
            },
            V1IdentityTargetPlan::New {
                username,
                display_name,
            } => {
                let p = secrets
                    .passwords
                    .iter_mut()
                    .find(|p| p.source_user_id == m.source_user_id)
                    .ok_or_else(|| {
                        Error::Validation("Fresh password required for new imported account".into())
                    })?;
                if !used.insert(p.source_user_id.clone()) {
                    return Err(Error::Conflict);
                }
                cowork_identity::MigrationIdentityTarget::New {
                    username: username.clone(),
                    display_name: display_name.clone(),
                    password: zeroize::Zeroizing::new(std::mem::take(&mut p.password)),
                }
            }
        };
        result.push(cowork_identity::MigrationIdentityMapping {
            source_user_id: m.source_user_id.clone(),
            target,
            role: m.role,
            expected_membership_revision: m.expected_membership_revision,
            project_grants: m
                .project_grants
                .iter()
                .map(|g| cowork_identity::MigrationProjectGrant {
                    project_id: g.project_id,
                    expected_resource_revision: g.expected_resource_revision,
                    expected_grant_revision: g.expected_grant_revision,
                    write: g.write,
                    share: g.share,
                })
                .collect(),
        });
    }
    if used.len() != secrets.passwords.len() {
        return Err(Error::Validation(
            "Unmapped or duplicate account secret".into(),
        ));
    }
    Ok(result)
}
pub(super) async fn configuration_bytes(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    team: Option<Uuid>,
    items: &[V1ImportedConfiguration],
) -> Result<u64> {
    let mut bytes = 0u64;
    for c in items {
        bytes = bytes
            .checked_add(json_string(c)?.len() as u64)
            .ok_or(Error::Conflict)?;
        if let Some(doc) = &c.document_id {
            let query = if team.is_some() {
                "SELECT payload FROM team_documents WHERE id=$1 AND project_id=$2"
            } else {
                "SELECT payload FROM productivity_objects WHERE id=$1 AND project_id=$2"
            };
            let payload: String = sqlx::query_scalar(query)
                .bind(doc)
                .bind(&c.project_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
            bytes = bytes
                .checked_add(payload.len() as u64)
                .ok_or(Error::Conflict)?;
        }
    }
    Ok(bytes)
}
pub type V1IdentityBinding = MigrationIdentityBinding;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1CurrentMember {
    pub identity_id: String,
    pub username: String,
    pub display_name: String,
    pub identity_revision: i64,
    pub membership_id: String,
    pub membership_revision: i64,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1CurrentProject {
    pub project_id: String,
    pub name: String,
    pub resource_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1CurrentGrant {
    pub project_id: String,
    pub membership_id: String,
    pub revision: i64,
    pub write: bool,
    pub share: bool,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1MappingOptions {
    pub members: Vec<V1CurrentMember>,
    pub projects: Vec<V1CurrentProject>,
    pub grants: Vec<V1CurrentGrant>,
    pub provider_revision: i64,
}
impl Store {
    pub async fn v1_team_migration_options(
        &self,
        actor: &cowork_identity::Principal,
        team: Uuid,
    ) -> Result<V1MappingOptions> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (members, projects, grants, provider_revision) = {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(crate::team_service::identity_error)?;
            locked
                .organization_in_transaction(actor, team)
                .await
                .map_err(crate::team_service::identity_error)?;
            let role:String=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1").bind(team.to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut **locked.transaction().map_err(crate::team_service::identity_error)?).await.map_err(storage_error)?;
            if !matches!(role.as_str(), "owner" | "admin") {
                return Err(Error::Forbidden);
            }
            let rows=sqlx::query("SELECT s.id AS identity_id,s.username,s.display_name,s.revision AS identity_revision,m.id AS membership_id,m.revision AS membership_revision,m.role FROM identity_memberships m JOIN identity_subjects s ON s.id=m.identity_id WHERE m.organization_id=$1 AND m.active=1 AND s.active=1 ORDER BY s.username").bind(team.to_string()).fetch_all(&mut **locked.transaction().map_err(crate::team_service::identity_error)?).await.map_err(storage_error)?;
            let mut members = vec![];
            for r in rows {
                members.push(V1CurrentMember {
                    identity_id: r.try_get("identity_id").map_err(storage_error)?,
                    username: r.try_get("username").map_err(storage_error)?,
                    display_name: r.try_get("display_name").map_err(storage_error)?,
                    identity_revision: r.try_get("identity_revision").map_err(storage_error)?,
                    membership_id: r.try_get("membership_id").map_err(storage_error)?,
                    membership_revision: r.try_get("membership_revision").map_err(storage_error)?,
                    role: r.try_get("role").map_err(storage_error)?,
                });
            }
            let rows=sqlx::query("SELECT p.id AS project_id,COALESCE(f.name,'Project') AS name,r.revision AS resource_revision FROM tenant_projects p JOIN identity_resources r ON r.id=p.id AND r.organization_id=p.organization_id LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.organization_id=$1 AND COALESCE(f.archived,0)=0").bind(team.to_string()).fetch_all(&mut **locked.transaction().map_err(crate::team_service::identity_error)?).await.map_err(storage_error)?;
            let mut projects = vec![];
            for r in rows {
                projects.push(V1CurrentProject {
                    project_id: r.try_get("project_id").map_err(storage_error)?,
                    name: r.try_get("name").map_err(storage_error)?,
                    resource_revision: r.try_get("resource_revision").map_err(storage_error)?,
                });
            }
            let rows=sqlx::query("SELECT g.resource_id,g.membership_id,g.revision,g.permissions,g.active FROM identity_content_grants g JOIN identity_resources r ON r.id=g.resource_id WHERE r.organization_id=$1 AND r.kind='project'").bind(team.to_string()).fetch_all(&mut **locked.transaction().map_err(crate::team_service::identity_error)?).await.map_err(storage_error)?;
            let mut grants = vec![];
            for r in rows {
                let bits: i64 = r.try_get("permissions").map_err(storage_error)?;
                grants.push(V1CurrentGrant {
                    project_id: r.try_get("resource_id").map_err(storage_error)?,
                    membership_id: r.try_get("membership_id").map_err(storage_error)?,
                    revision: r.try_get("revision").map_err(storage_error)?,
                    write: bits & 2 != 0,
                    share: bits & 8 != 0,
                    active: r.try_get::<i64, _>("active").map_err(storage_error)? == 1,
                });
            }
            let provider_revision =
                sqlx::query_scalar("SELECT revision FROM team_provider WHERE team_id=$1")
                    .bind(team.to_string())
                    .fetch_optional(
                        &mut **locked
                            .transaction()
                            .map_err(crate::team_service::identity_error)?,
                    )
                    .await
                    .map_err(storage_error)?
                    .unwrap_or(0);
            (members, projects, grants, provider_revision)
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(V1MappingOptions {
            members,
            projects,
            grants,
            provider_revision,
        })
    }
    pub async fn v1_team_imported_configurations(
        &self,
        actor: &cowork_identity::Principal,
        team: Uuid,
        project: Uuid,
    ) -> Result<Vec<V1ImportedConfiguration>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let result = {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(crate::team_service::identity_error)?;
            let permit = locked
                .authorize_content_in_transaction(
                    actor,
                    team,
                    project,
                    cowork_identity::ContentAction::Read,
                )
                .await
                .map_err(crate::team_service::identity_error)?;
            let rows=sqlx::query("SELECT payload FROM v1_migration_configurations WHERE scope='team' AND team_id=$1 AND project_id=$2 ORDER BY id").bind(team.to_string()).bind(project.to_string()).fetch_all(&mut **locked.transaction().map_err(crate::team_service::identity_error)?).await.map_err(storage_error)?;
            let result = rows
                .into_iter()
                .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
                .collect::<Result<_>>()?;
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(crate::team_service::identity_error)?;
            result
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
pub(super) fn validate_mapping(
    source: &V1ImportPreview,
    mapping: &V1MigrationMapping,
) -> Result<()> {
    if mapping.users.len() > 1000
        || mapping.project_destinations.len() > 100
        || mapping.selected_object_ids.len() > 10000
    {
        return Err(Error::Conflict);
    }
    let mut users = std::collections::BTreeSet::new();
    for m in &mapping.users {
        if !source
            .catalog
            .users
            .iter()
            .any(|u| u.source_id == m.source_user_id)
            || !users.insert(&m.source_user_id)
            || m.project_grants.len() > 100
        {
            return Err(Error::Conflict);
        }
        if let V1IdentityTargetPlan::New {
            username,
            display_name,
        } = &m.target
        {
            valid_text(username, 200)?;
            valid_text(display_name, 200)?;
        }
    }
    let mut projects = std::collections::BTreeSet::new();
    for d in &mapping.project_destinations {
        if !source
            .projects
            .iter()
            .any(|p| p.source_id == d.source_project_id)
            || !projects.insert(&d.source_project_id)
        {
            return Err(Error::Conflict);
        }
    }
    let available = source
        .catalog
        .objects
        .iter()
        .map(|o| format!("{}:{}", o.kind, o.source_id))
        .collect::<std::collections::BTreeSet<_>>();
    let selected = mapping
        .selected_object_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    if selected.len() != mapping.selected_object_ids.len()
        || selected.iter().any(|s| !available.contains(*s))
    {
        return Err(Error::Conflict);
    }
    if let Some(p) = &mapping.provider {
        if p.expected_revision < 0
            || !source
                .catalog
                .objects
                .iter()
                .any(|o| o.kind == "provider" && o.source_id == p.source_id)
        {
            return Err(Error::Conflict);
        }
        valid_text(&p.model, 256)?;
        valid_text(&p.base_url, 8192)?;
    }
    Ok(())
}
