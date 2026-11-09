use super::{
    Budget, Error, MAPPING_FORMAT, MAX_MANIFEST, MAX_PREVIEW, MAX_ROWS, PREVIEW_FORMAT, Result,
    canonical as c, policy, quote, uuid,
};
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityKind {
    Project,
    Thread,
    EngineRun,
    CrewRun,
    ArtifactVersion,
    EngineArtifact,
    AgentPersona,
    DaemonRun,
    HistoricalActor,
}
impl EntityKind {
    pub(super) fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "project" => Self::Project,
            "thread" => Self::Thread,
            "engine_run" => Self::EngineRun,
            "crew_run" => Self::CrewRun,
            "artifact_version" => Self::ArtifactVersion,
            "engine_artifact" => Self::EngineArtifact,
            "agent_persona" => Self::AgentPersona,
            "daemon_run" => Self::DaemonRun,
            "historical_actor" => Self::HistoricalActor,
            _ => return Err(Error::Refused("Unknown original mapping kind")),
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Thread => "thread",
            Self::EngineRun => "engine_run",
            Self::CrewRun => "crew_run",
            Self::ArtifactVersion => "artifact_version",
            Self::EngineArtifact => "engine_artifact",
            Self::AgentPersona => "agent_persona",
            Self::DaemonRun => "daemon_run",
            Self::HistoricalActor => "historical_actor",
        }
    }
    fn target_kind(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::EngineRun | Self::CrewRun | Self::DaemonRun => "run",
            Self::ArtifactVersion | Self::EngineArtifact => "artifact",
            Self::HistoricalActor => "identity_subject",
            _ => self.as_str(),
        }
    }
    fn history_only(self) -> bool {
        matches!(
            self,
            Self::HistoricalActor | Self::AgentPersona | Self::Thread | Self::CrewRun
        )
    }
    fn artifact(self) -> bool {
        matches!(self, Self::ArtifactVersion | Self::EngineArtifact)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryNamespace {
    History,
    Personal,
    Tenant {
        organization: Uuid,
        creator_identity: Uuid,
    },
}
/// References an old cold snapshot only. This cannot substitute current row pins,
/// authenticated Principal, ContentWrite, byte quota or a caller transaction.
#[derive(Clone, Debug)]
pub struct HistoricalMapping {
    source_kind: EntityKind,
    source_id: String,
    target_id: Uuid,
    namespace: HistoryNamespace,
    project_id: Option<Uuid>,
    cold_target_facts: Option<Value>,
}
impl HistoricalMapping {
    pub fn source_kind(&self) -> EntityKind {
        self.source_kind
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn target_id(&self) -> Uuid {
        self.target_id
    }
    pub fn namespace(&self) -> &HistoryNamespace {
        &self.namespace
    }
    pub fn project_id(&self) -> Option<Uuid> {
        self.project_id
    }
    pub fn historical_cold_target_facts(&self) -> Option<&Value> {
        self.cold_target_facts.as_ref()
    }
}
type KnownIds = BTreeMap<String, BTreeSet<String>>;

async fn known_ids(
    target: &mut SqliteConnection,
    fixed: &Value,
    wanted: &BTreeSet<String>,
    budget: Budget,
) -> Result<KnownIds> {
    let mut result = KnownIds::new();
    let tables = fixed["historical_target"]["schema"]["tables"]
        .as_object()
        .ok_or(Error::Refused("Fixed target tables"))?;
    let uuid_tables = [
        "projects",
        "tenant_projects",
        "runs",
        "artifacts",
        "orders",
        "identity_resources",
        "identity_subjects",
        "identity_organizations",
    ];
    let mut count = 0;
    for (table, metadata) in tables {
        if !metadata["columns"]
            .as_array()
            .ok_or(Error::Refused("Fixed target columns"))?
            .iter()
            .any(|column| column[1] == "id")
        {
            continue;
        }
        let query = format!("SELECT id,typeof(id) FROM {}", quote(table));
        let mut cursor = sqlx::query(&query).fetch(&mut *target);
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            count += 1;
            if count > MAX_ROWS {
                return Err(Error::Refused("Cold target opaque ID count limit"));
            }
            let storage: String = row.try_get(1)?;
            // New qualified Rust semantic rule, not legacy Python parity: the one
            // known BIGINT CHECK(id=1) installation row is never an entity UUID.
            if table == "identity_installation" {
                if storage != "integer" || row.try_get::<i64, _>(0)? != 1 {
                    return Err(Error::Refused(
                        "Known installation singleton must be INTEGER id=1",
                    ));
                }
                continue;
            }
            if storage != "text" {
                return Err(Error::Refused("Unknown cold target ID storage class"));
            }
            let id: String = row.try_get(0)?;
            if uuid_tables.contains(&table.as_str()) {
                uuid(&json!(&id))?;
            } else {
                c::identifier(&id)?;
            }
            if wanted.contains(&id) {
                result.entry(id).or_default().insert(table.clone());
            }
        }
    }
    Ok(result)
}

fn scope_equal(a: &Value, b: &Value) -> bool {
    [
        "namespace",
        "project_id",
        "organization_id",
        "creator_identity_id",
    ]
    .iter()
    .all(|key| a[*key] == b[*key])
}
fn exactly(existing: &BTreeSet<String>, tables: &[&str]) -> bool {
    existing.len() == tables.len() && tables.iter().all(|name| existing.contains(*name))
}

pub(super) async fn verify(
    source: &mut SqliteConnection,
    target: &mut SqliteConnection,
    mapping: &Value,
    preview: &Value,
    fixed: &Value,
    pins: &super::HistoricalPins,
    budget: Budget,
) -> Result<Vec<HistoricalMapping>> {
    let export_sha = pins.export_manifest.as_str();
    let quarantine_sha = pins.quarantine_manifest.as_str();
    let target_sha = pins.cold_target_snapshot.as_str();
    let mapping_sha = pins.mapping.as_str();
    c::object(
        mapping,
        &[
            "format",
            "purpose",
            "export_sha256",
            "target_snapshot_sha256",
            "entries",
        ],
    )?;
    if mapping["format"] != MAPPING_FORMAT
        || mapping["purpose"] != "historical_preview"
        || mapping["export_sha256"] != export_sha
        || mapping["target_snapshot_sha256"] != target_sha
    {
        return Err(Error::Refused(
            "Mapping binds another source/snapshot/purpose",
        ));
    }
    let entries = mapping["entries"]
        .as_array()
        .ok_or(Error::Refused("Mapping entries"))?;
    if entries.len() > MAX_ROWS as usize {
        return Err(Error::Refused("Mapping entry limit"));
    }
    let mut mapped = BTreeMap::new();
    let mut destinations = BTreeSet::new();
    let mut wanted = BTreeSet::new();
    for entry in entries {
        c::object(
            entry,
            &[
                "source_kind",
                "source_id",
                "target_kind",
                "target_id",
                "namespace",
                "project_id",
                "organization_id",
                "creator_identity_id",
            ],
        )?;
        let kind = EntityKind::parse(c::string(&entry["source_kind"])?)?;
        let id = c::identifier(c::string(&entry["source_id"])?)?;
        if mapped.insert((kind, id.to_owned()), entry).is_some() {
            return Err(Error::Refused("Duplicate/multivalued original mapping"));
        }
        let dest = uuid(&entry["target_id"])?;
        if !destinations.insert(dest) {
            return Err(Error::Refused(
                "Multiple original entities target same V2 ID",
            ));
        }
        if entry["target_kind"] != kind.target_kind() {
            return Err(Error::Refused("Human identity/persona mapping conflation"));
        }
        wanted.insert(dest.to_string());
        for field in ["project_id", "organization_id", "creator_identity_id"] {
            if !entry[field].is_null() {
                wanted.insert(uuid(&entry[field])?.to_string());
            }
        }
    }
    let known = known_ids(target, fixed, &wanted, budget).await?;
    let empty = BTreeSet::new();
    let mut results = Vec::new();
    let mut typed = Vec::new();
    for entry in entries {
        budget.check()?;
        let kind = EntityKind::parse(c::string(&entry["source_kind"])?)?;
        let id = c::string(&entry["source_id"])?;
        let dest = uuid(&entry["target_id"])?;
        let original=sqlx::query("SELECT project_source_id,actor_source_id FROM history_entities WHERE kind=? AND source_id=?").bind(kind.as_str()).bind(id).fetch_optional(&mut *source).await?.ok_or(Error::Refused("Mapping references absent original ID"))?;
        let original_project: Option<String> = original.try_get(0)?;
        let original_actor: Option<String> = original.try_get(1)?;
        let namespace = c::string(&entry["namespace"])?;
        let (scope, project) = if kind.history_only() {
            if namespace != "history"
                || ["project_id", "organization_id", "creator_identity_id"]
                    .iter()
                    .any(|field| !entry[*field].is_null())
            {
                return Err(Error::Refused(
                    "Actor/persona/thread remains nonauthoritative history",
                ));
            }
            (HistoryNamespace::History, None)
        } else {
            let project = uuid(&entry["project_id"])?;
            if kind == EntityKind::Project && project != dest {
                return Err(Error::Refused("Project target/project pin differs"));
            }
            let scope = match namespace {
                "personal" => {
                    if !entry["organization_id"].is_null()
                        || !entry["creator_identity_id"].is_null()
                    {
                        return Err(Error::Refused("Personal history inherits tenant identity"));
                    }
                    HistoryNamespace::Personal
                }
                "tenant" => {
                    let org = uuid(&entry["organization_id"])?;
                    let actor = uuid(&entry["creator_identity_id"])?;
                    if !known
                        .get(&org.to_string())
                        .is_some_and(|s| s.contains("identity_organizations"))
                        || !known
                            .get(&actor.to_string())
                            .is_some_and(|s| s.contains("identity_subjects"))
                    {
                        return Err(Error::Refused(
                            "Tenant mapping requires existing pinned organization/creator",
                        ));
                    }
                    HistoryNamespace::Tenant {
                        organization: org,
                        creator_identity: actor,
                    }
                }
                _ => {
                    return Err(Error::Refused(
                        "Explicit personal/tenant namespace required",
                    ));
                }
            };
            (scope, Some(project))
        };
        let dest_string = dest.to_string();
        let existing = known.get(&dest_string).unwrap_or(&empty);
        let mut facts = None;
        match kind {
            EntityKind::Project if namespace == "personal" => {
                if !existing.is_empty() && !exactly(existing, &["projects"]) {
                    return Err(Error::Refused(
                        "Personal target ID belongs to another namespace",
                    ));
                }
                if let Some(row) = sqlx::query("SELECT payload FROM projects WHERE id=?")
                    .bind(&dest_string)
                    .fetch_optional(&mut *target)
                    .await?
                {
                    let payload: String = row.try_get(0)?;
                    facts = Some(
                        json!({"namespace":"personal","payload_sha256":c::hex(&c::hash(payload.as_bytes()))}),
                    );
                }
            }
            EntityKind::Project => {
                if !existing.is_empty()
                    && !exactly(existing, &["tenant_projects", "identity_resources"])
                {
                    return Err(Error::Refused(
                        "Tenant project cannot adopt unrelated resource",
                    ));
                }
                if let Some(row)=sqlx::query("SELECT p.organization_id,p.creator_identity_id,p.revision,r.organization_id,r.kind,r.revision,p.creation_key,p.request_sha256,p.creation_payload,p.payload,p.initial_receipt FROM tenant_projects p JOIN identity_resources r ON r.id=p.id WHERE p.id=?").bind(&dest_string).fetch_optional(&mut *target).await?{
                    let org:String=row.try_get(0)?;let actor:String=row.try_get(1)?;let resource_org:String=row.try_get(3)?;let resource_kind:String=row.try_get(4)?;
                    if entry["organization_id"]!=org||entry["creator_identity_id"]!=actor||resource_org!=org||resource_kind!="project"{return Err(Error::Refused("Tenant org/kind/creator differs"));}
                    let creation=c::parse(row.try_get::<String,_>(8)?.as_bytes(),MAX_MANIFEST)?;let payload=c::parse(row.try_get::<String,_>(9)?.as_bytes(),MAX_MANIFEST)?;let initial=c::parse(row.try_get::<String,_>(10)?.as_bytes(),MAX_MANIFEST)?;
                    c::object(&creation,&["name","idempotency_key"])?;c::object(&payload,&["id","organization_id","name","revision","created_at"])?;c::object(&initial,&["id","organization_id","name","revision","created_at"])?;
                    let name=c::string(&creation["name"])?;let intent=c::string(&creation["idempotency_key"])?;
                    if name.trim().is_empty()||name.len()>200||intent.trim().is_empty()||intent.len()>128||name.chars().chain(intent.chars()).any(|ch|ch<' '||ch=='\u{7f}') {return Err(Error::Refused("Original tenant creation name/key invalid"));}
                    // The existing immutable request hashes JSON object fields in
                    // declaration order (name, then idempotency_key), not sorted
                    // Value-map order. Preserve that exact historical contract.
                    #[derive(serde::Serialize)]
                    struct Creation<'a>{name:&'a str,idempotency_key:&'a str}
                    let request=serde_json::to_vec(&("tenant_project_create/v1",&org,&actor,Creation{name,idempotency_key:intent})).map_err(|_|Error::Refused("Tenant creation serialization"))?;
                    let revision:i64=row.try_get(2)?;let resource_revision:i64=row.try_get(5)?;let key:String=row.try_get(6)?;let request_sha:String=row.try_get(7)?;
                    if payload!=initial||payload["id"]!=dest_string||payload["organization_id"]!=org||payload["revision"]!=json!(revision)||revision!=1||resource_revision<=0||payload["created_at"].as_i64().is_none_or(|n|n<=0)||payload["name"]!=name.trim()||key!=intent||request_sha!=c::hex(&c::hash(&request)){return Err(Error::Refused("Tenant payload/immutable creation receipt differs"));}
                    facts=Some(json!({"namespace":"tenant","organization_id":org,"creator_identity_id":actor,"project_revision":revision,"resource_revision":resource_revision}));
                }
            }
            EntityKind::EngineRun | EntityKind::DaemonRun => {
                if !existing.is_empty() && !exactly(existing, &["runs"]) {
                    return Err(Error::Refused(
                        "Run target ID already belongs to another entity",
                    ));
                }
                if let Some(row)=sqlx::query("SELECT o.project_id,r.revision FROM runs r JOIN orders o ON o.id=r.order_id WHERE r.id=?").bind(&dest_string).fetch_optional(&mut *target).await?{
                    let project:String=row.try_get(0)?;let revision:i64=row.try_get(1)?;if namespace!="personal"||entry["project_id"]!=project||revision<0{return Err(Error::Refused("Run target belongs to another project/namespace"));}facts=Some(json!({"project_id":project,"revision":revision}));}
            }
            EntityKind::ArtifactVersion | EntityKind::EngineArtifact => {
                if !existing.is_empty() && !exactly(existing, &["artifacts"]) {
                    return Err(Error::Refused(
                        "Artifact target ID belongs to another namespace",
                    ));
                }
                if let Some(row)=sqlx::query("SELECT o.project_id,a.run_id FROM artifacts a JOIN orders o ON o.id=a.order_id WHERE a.id=?").bind(&dest_string).fetch_optional(&mut *target).await?{
                    let project:String=row.try_get(0)?;let run:String=row.try_get(1)?;if namespace!="personal"||entry["project_id"]!=project{return Err(Error::Refused("Artifact target belongs to another project"));}facts=Some(json!({"project_id":project,"run_id":run}));}
            }
            EntityKind::HistoricalActor => {
                if !exactly(existing, &["identity_subjects"]) {
                    return Err(Error::Refused(
                        "Historical actor may only refer to existing human ID",
                    ));
                }
                let row = sqlx::query("SELECT active,revision FROM identity_subjects WHERE id=?")
                    .bind(&dest_string)
                    .fetch_one(&mut *target)
                    .await?;
                let active: i64 = row.try_get(0)?;
                let revision: i64 = row.try_get(1)?;
                if !(0..=1).contains(&active) || revision <= 0 {
                    return Err(Error::Refused("Target human metadata invalid"));
                }
                facts = Some(json!({"active":active,"revision":revision}));
            }
            _ => {
                if !existing.is_empty() {
                    return Err(Error::Refused(
                        "Historical content/persona collides with operational entity",
                    ));
                }
            }
        }
        results.push(json!({"mapping":entry,"diff":if facts.is_some(){"existing_target_reference"}else{"proposed_new_history_id"},"target_facts":facts,
                           "source_project_id":original_project,"source_actor_id":original_actor,"grants_created":0,"activation_allowed":false}));
        typed.push(HistoricalMapping {
            source_kind: kind,
            source_id: id.to_owned(),
            target_id: dest,
            namespace: scope,
            project_id: project,
            cold_target_facts: facts,
        });
    }
    for ((kind, id), entry) in &mapped {
        let row=sqlx::query("SELECT project_source_id,actor_source_id FROM history_entities WHERE kind=? AND source_id=?").bind(kind.as_str()).bind(id).fetch_one(&mut *source).await?;
        let project: Option<String> = row.try_get(0)?;
        let actor: Option<String> = row.try_get(1)?;
        if entry["namespace"] != "history" && *kind != EntityKind::Project {
            let project =
                project.ok_or(Error::Refused("Mapped content original project missing"))?;
            let project = mapped
                .get(&(EntityKind::Project, project))
                .ok_or(Error::Refused("Mapped original project missing"))?;
            if !scope_equal(entry, project) {
                return Err(Error::Refused(
                    "Content mapping crosses original project pins",
                ));
            }
            if entry["namespace"] == "tenant"
                && let Some(actor) = actor
            {
                let actor = mapped
                    .get(&(EntityKind::HistoricalActor, actor))
                    .ok_or(Error::Refused("Original historical actor mapping missing"))?;
                if actor["target_id"] != entry["creator_identity_id"] {
                    return Err(Error::Refused(
                        "Tenant creator differs from mapped original actor",
                    ));
                }
            }
        }
        if kind.artifact() {
            let linked=sqlx::query("SELECT source_kind,source_id FROM history_relations WHERE kind='run_artifact' AND target_kind=? AND target_id=?").bind(kind.as_str()).bind(id).fetch_all(&mut *source).await?;
            if linked.len() != 1 {
                return Err(Error::Refused(
                    "Artifact exact original run relation missing",
                ));
            }
            let key = (
                EntityKind::parse(&linked[0].try_get::<String, _>(0)?)?,
                linked[0].try_get::<String, _>(1)?,
            );
            let run = mapped
                .get(&key)
                .ok_or(Error::Refused("Artifact original run mapping missing"))?;
            if !scope_equal(entry, run) {
                return Err(Error::Refused("Artifact crosses mapped run/project pins"));
            }
            if let Some(row) = sqlx::query("SELECT run_id FROM artifacts WHERE id=?")
                .bind(c::string(&entry["target_id"])?)
                .fetch_optional(&mut *target)
                .await?
                && json!(row.try_get::<String, _>(0)?) != run["target_id"]
            {
                return Err(Error::Refused(
                    "Existing artifact belongs to another mapped run",
                ));
            }
        }
    }
    let mut unmapped = Vec::new();
    let mut all = 0;
    let mut byte_budget = 0;
    let mut cursor =
        sqlx::query("SELECT kind,source_id FROM history_entities ORDER BY kind,source_id")
            .fetch(&mut *source);
    while let Some(row) = cursor.try_next().await? {
        budget.check()?;
        all += 1;
        if all > MAX_ROWS * 3 {
            return Err(Error::Refused("Historical entity limit"));
        }
        let name: String = row.try_get(0)?;
        let id: String = row.try_get(1)?;
        let kind = EntityKind::parse(&name)?;
        if !mapped.contains_key(&(kind, id.clone())) {
            let value = json!({"source_kind":name,"source_id":id});
            byte_budget += c::canonical(&value)?.len();
            if byte_budget > MAX_PREVIEW {
                return Err(Error::Refused("Preview diff byte limit"));
            }
            unmapped.push(value);
        }
    }
    let diff = json!({"entries":results,"unmapped":unmapped,"complete_mapping":all as usize==mapped.len(),"fresh_principal_required_for_activation":true});
    c::object(
        preview,
        &[
            "format",
            "purpose",
            "state",
            "execution_policy",
            "quarantine_sha256",
            "export_sha256",
            "target_snapshot_sha256",
            "mapping_sha256",
            "target_profile",
            "diff",
            "target_sessions_queried_for_authority",
            "target_credentials_exported",
            "target_database_writes",
            "grants_created",
            "activation_allowed",
            "manifest_sha256",
        ],
    )?;
    c::sealed(preview)?;
    let target_profile = &fixed["historical_target"];
    let expected_profile = json!({"profile":target_profile["profile"],"schema_sha256":target_profile["schema_sha256"],"source_bindings":target_profile["source_bindings"]});
    if preview["format"] != PREVIEW_FORMAT
        || preview["purpose"] != "historical_preview"
        || preview["state"] != "inactive"
        || preview["execution_policy"] != policy()
        || preview["quarantine_sha256"] != quarantine_sha
        || preview["export_sha256"] != export_sha
        || preview["target_snapshot_sha256"] != target_sha
        || preview["mapping_sha256"] != mapping_sha
        || preview["target_profile"] != expected_profile
        || preview["diff"] != diff
        || preview["target_sessions_queried_for_authority"] != false
        || preview["target_credentials_exported"] != false
        || preview["target_database_writes"] != 0
        || preview["grants_created"] != 0
        || preview["activation_allowed"] != false
    {
        return Err(Error::Refused(
            "Preview provenance/diff/policy differs from complete rederived history",
        ));
    }
    Ok(typed)
}
