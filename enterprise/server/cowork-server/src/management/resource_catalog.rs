//! Persistent configuration and explicit grants; delivery is not runtime enforcement.
use super::*;
use axum::{extract::Query, http::StatusCode};
use serde::Serialize;
use std::collections::HashSet;
pub(super) mod model_proxy;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Model,
    Endpoint,
    Mcp,
    Skill,
    AgentRole,
}
impl Kind {
    fn text(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Endpoint => "endpoint",
            Self::Mcp => "mcp",
            Self::Skill => "skill",
            Self::AgentRole => "agent_role",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceInput {
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub configuration: Value,
    pub secret_reference: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateResource {
    pub expected_revision: i64,
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub configuration: Value,
    pub secret_reference: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteResource {
    pub expected_revision: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelConfig {
    model_id: String,
    endpoint_resource_id: Option<Uuid>,
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
    pricing: Option<ModelPricing>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelPricing {
    currency: String,
    input_micros_per_million: u64,
    output_micros_per_million: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EndpointConfig {
    base_url: String,
    protocol: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct McpConfig {
    transport: String,
    url: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SkillConfig {
    instructions: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentConfig {
    system_prompt: String,
    #[serde(default)]
    resource_ids: Vec<Uuid>,
}

fn nonsecret(value: &str, max: usize) -> Result<(), ApiError> {
    let lower = value.to_ascii_lowercase();
    let credential = value
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ':' | ','))
        .any(|word| (word.starts_with("sk-") || word.starts_with("sk_")) && word.len() > 16);
    if value.len() > max
        || value.contains('\0')
        || credential
        || [
            "bearer ",
            "-----begin",
            "api_key=",
            "apikey=",
            "access_token=",
            "password=",
        ]
        .iter()
        .any(|part| lower.contains(*part))
    {
        return Err(invalid(
            "configuration must contain non-secret metadata only; use a secret_reference",
        ));
    }
    Ok(())
}
fn address(value: &str) -> Result<(), ApiError> {
    nonsecret(value, 2048)?;
    let url = reqwest::Url::parse(value)
        .map_err(|_| invalid("use an HTTP(S) endpoint without credentials, query or fragment"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "use an HTTP(S) endpoint without credentials, query or fragment",
        ));
    }
    Ok(())
}
fn secret_reference(value: &Option<String>) -> Result<(), ApiError> {
    if let Some(value) = value {
        let Some((prefix, path)) = value.split_once(':') else {
            return Err(invalid("secret_reference must use env:, vault: or secret:"));
        };
        if !matches!(prefix, "env" | "vault" | "secret")
            || path.is_empty()
            || path.len() > 180
            || !path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))
        {
            return Err(invalid(
                "secret_reference must be a non-secret env:, vault: or secret: identifier",
            ));
        }
    }
    Ok(())
}
fn validate(input: &mut ResourceInput) -> Result<Vec<Uuid>, ApiError> {
    input.name = name(&input.name)?.to_owned();
    nonsecret(&input.name, 120)?;
    nonsecret(&input.description, 4000)?;
    secret_reference(&input.secret_reference)?;
    if serde_json::to_vec(&input.configuration)?.len() > 32768 {
        return Err(invalid("resource configuration exceeds 32 KiB"));
    }
    let mut dependencies = vec![];
    input.configuration = match input.kind {
        Kind::Model => {
            let c:ModelConfig=serde_json::from_value(input.configuration.clone()).map_err(|_|invalid("model configuration needs model_id and optional endpoint_resource_id/context_window/max_output_tokens"))?;
            if c.model_id.trim().is_empty()
                || c.context_window == Some(0)
                || c.max_output_tokens == Some(0)
                || c.context_window.is_some_and(|v| v > 1_000_000)
                || c.max_output_tokens.is_some_and(|v| v > 131_072)
                || matches!((c.context_window,c.max_output_tokens),(Some(context),Some(output)) if output >= context)
            {
                return Err(invalid(
                    "model identifiers and token limits must be nonempty/positive",
                ));
            }
            nonsecret(&c.model_id, 200)?;
            if let Some(pricing) = &c.pricing {
                if pricing.currency.len() != 3
                    || !pricing.currency.bytes().all(|b| b.is_ascii_uppercase())
                    || pricing.input_micros_per_million > 1_000_000_000_000
                    || pricing.output_micros_per_million > 1_000_000_000_000
                {
                    return Err(invalid("pricing needs an explicit three-letter currency and bounded microcurrency rates per million tokens"));
                }
            }
            dependencies.extend(c.endpoint_resource_id);
            serde_json::to_value(c)?
        }
        Kind::Endpoint => {
            let c: EndpointConfig = serde_json::from_value(input.configuration.clone())
                .map_err(|_| invalid("endpoint configuration needs base_url and protocol"))?;
            address(&c.base_url)?;
            if !["openai_compatible", "ollama", "anthropic", "azure_openai"]
                .contains(&c.protocol.as_str())
            {
                return Err(invalid("unsupported endpoint protocol"));
            }
            serde_json::to_value(c)?
        }
        Kind::Mcp => {
            let c: McpConfig =
                serde_json::from_value(input.configuration.clone()).map_err(|_| {
                    invalid("MCP configuration needs transport and url or command/args")
                })?;
            match c.transport.as_str() {
                "streamable_http" => {
                    address(
                        c.url
                            .as_deref()
                            .ok_or_else(|| invalid("HTTP MCP needs url"))?,
                    )?;
                    if c.command.is_some() || !c.args.is_empty() {
                        return Err(invalid("HTTP MCP cannot contain command/args"));
                    }
                }
                "stdio" => {
                    let command = c
                        .command
                        .as_deref()
                        .ok_or_else(|| invalid("stdio MCP needs command"))?;
                    nonsecret(command, 512)?;
                    if command.trim().is_empty() || c.url.is_some() || c.args.len() > 64 {
                        return Err(invalid("invalid stdio MCP configuration"));
                    }
                    for arg in &c.args {
                        nonsecret(arg, 1024)?;
                        if [
                            "token",
                            "secret",
                            "password",
                            "api-key",
                            "api_key",
                            "authorization",
                        ]
                        .iter()
                        .any(|key| arg.to_ascii_lowercase().contains(*key))
                        {
                            return Err(invalid("MCP credential arguments must be supplied by the secret reference resolver"));
                        }
                    }
                }
                _ => return Err(invalid("MCP transport must be streamable_http or stdio")),
            }
            serde_json::to_value(c)?
        }
        Kind::Skill => {
            let c: SkillConfig = serde_json::from_value(input.configuration.clone())
                .map_err(|_| invalid("skill configuration needs instructions"))?;
            nonsecret(&c.instructions, 16384)?;
            if c.instructions.trim().is_empty() {
                return Err(invalid("skill instructions cannot be empty"));
            }
            serde_json::to_value(c)?
        }
        Kind::AgentRole => {
            let c: AgentConfig =
                serde_json::from_value(input.configuration.clone()).map_err(|_| {
                    invalid(
                        "agent_role configuration needs system_prompt and optional resource_ids",
                    )
                })?;
            nonsecret(&c.system_prompt, 16384)?;
            if c.system_prompt.trim().is_empty() || c.resource_ids.len() > 100 {
                return Err(invalid("invalid agent role prompt/resource list"));
            }
            dependencies.extend(&c.resource_ids);
            serde_json::to_value(c)?
        }
    };
    let mut seen = HashSet::new();
    if dependencies
        .iter()
        .any(|id| id.is_nil() || !seen.insert(*id))
    {
        return Err(invalid(
            "resource dependencies must be distinct non-nil IDs",
        ));
    }
    Ok(dependencies)
}
async fn platform_guard(
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
) -> Result<(), ApiError> {
    let admin: Option<bool> = sqlx::query_scalar(
        "SELECT platform_admin FROM users WHERE id=$1 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(principal.user_id)
    .fetch_optional(&mut **tx)
    .await?;
    if principal.bootstrap || admin != Some(true) {
        return Err(denied());
    }
    Ok(())
}
async fn mutation_lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), ApiError> {
    // Catalog deletion also advances assignment revisions. A shared mutation
    // order prevents set-row/resource-row inversion and stale replacement races.
    sqlx::query("SELECT pg_advisory_xact_lock(7100373)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn org_guard(
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
    admin: bool,
) -> Result<bool, ApiError> {
    super::organisation_lifecycle::lock_active(tx, org).await?;
    let platform: Option<bool> = sqlx::query_scalar(
        "SELECT platform_admin FROM users WHERE id=$1 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(principal.user_id)
    .fetch_optional(&mut **tx)
    .await?;
    if principal.bootstrap || platform.is_none() {
        return Err(denied());
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM management_members WHERE organisation_id=$1 AND user_id=$2 FOR SHARE",
    )
    .bind(org)
    .bind(principal.user_id)
    .fetch_optional(&mut **tx)
    .await?;
    let administrator = platform == Some(true) || role.as_deref() == Some("admin");
    if !administrator
        && (admin
            || !role
                .as_deref()
                .is_some_and(|r| matches!(r, "member" | "auditor")))
    {
        return Err(denied());
    }
    Ok(administrator)
}
async fn dependencies(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: &ResourceInput,
    ids: &[Uuid],
) -> Result<(), ApiError> {
    for dependency in ids {
        if *dependency == id {
            return Err(invalid("a resource cannot depend on itself"));
        }
        let kind:Option<String>=sqlx::query_scalar("SELECT kind FROM management_resource_catalog WHERE id=$1 AND deleted_at IS NULL AND management_resource_available(id) FOR SHARE").bind(dependency).fetch_optional(&mut **tx).await?;
        let valid = match (input.kind, kind.as_deref()) {
            (Kind::Model, Some("endpoint")) => true,
            (Kind::AgentRole, Some(k)) => k != "agent_role",
            _ => false,
        };
        if !valid {
            return Err(invalid("dependency must be an available endpoint (model) or non-agent-role resource (agent role)"));
        }
    }
    sqlx::query("DELETE FROM management_resource_dependencies WHERE resource_id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for dependency in ids {
        sqlx::query(
            "INSERT INTO management_resource_dependencies(resource_id,depends_on) VALUES($1,$2)",
        )
        .bind(id)
        .bind(dependency)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
fn delivery() -> Value {
    json!({"state":"partial_consumers","enforced":false,"message":"Upgraded server agents consume selected central skills, agent roles and MCP resources with revision-bound checks before model/tool dispatch. Organisation and instance scopes are supported; bound environment, Crew and device consumers require separate qualification. MCP secret references require an explicit execution-target binding. OpenAI-compatible model credentials remain in the management proxy. Wallet reservations apply to managed proxy requests; local profiles retain their own limits.",
        "supported_model_subset":{"consumer_kind":"model_provider","version":1,"protocols":["openai_compatible"],"scopes":["organisation","instance"],"enforcement":"fresh_resolution_only","requires_instance_upgrade":true,"immediate_server_runs_only":true,"wallet_proxy_version":1,"wallet_scope":"managed_proxy_requests","pricing_required_when_wallet_enabled":true,"provider_secret_location":"management"}})
}
async fn record(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Value, ApiError> {
    sqlx::query_scalar("SELECT to_jsonb(r)||jsonb_build_object('available',management_resource_available(r.id)) FROM management_resource_catalog r WHERE r.id=$1 AND r.deleted_at IS NULL").bind(id).fetch_optional(&mut **tx).await?.ok_or_else(denied)
}
async fn catalog_audit(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    id: Uuid,
    action: &str,
    metadata: Value,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events(id,actor_user_id,action,target_type,target_id,metadata) VALUES($1,$2,$3,'management_resource',$4,$5)").bind(Uuid::new_v4()).bind(actor).bind(action).bind(id).bind(metadata).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    platform_guard(&mut tx, &principal).await?;
    let items:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r)||jsonb_build_object('available',management_resource_available(r.id)) FROM management_resource_catalog r WHERE deleted_at IS NULL ORDER BY kind,name,id LIMIT 2000").fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"items":items,"delivery":delivery()})))
}
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(mut input): Json<ResourceInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let ids = validate(&mut input)?;
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    mutation_lock(&mut tx).await?;
    platform_guard(&mut tx, &principal).await?;
    sqlx::query("INSERT INTO management_resource_catalog(id,kind,name,description,configuration,secret_reference,enabled,created_by,updated_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$8)").bind(id).bind(input.kind.text()).bind(&input.name).bind(&input.description).bind(&input.configuration).bind(&input.secret_reference).bind(input.enabled).bind(principal.user_id).execute(&mut *tx).await?;
    dependencies(&mut tx, id, &input, &ids).await?;
    catalog_audit(
        &mut tx,
        principal.user_id,
        id,
        "resource_catalog.created",
        json!({"kind":input.kind,"revision":1,"enabled":input.enabled}),
    )
    .await?;
    let value = record(&mut tx, id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(value)))
}
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateResource>,
) -> Result<Json<Value>, ApiError> {
    let expected_revision = request.expected_revision;
    if expected_revision < 1 {
        return Err(invalid("expected_revision must be positive"));
    }
    let mut input = ResourceInput {
        kind: request.kind,
        name: request.name,
        description: request.description,
        configuration: request.configuration,
        secret_reference: request.secret_reference,
        enabled: request.enabled,
    };
    let ids = validate(&mut input)?;
    let mut tx = state.pool.begin().await?;
    mutation_lock(&mut tx).await?;
    platform_guard(&mut tx, &principal).await?;
    let current=sqlx::query("SELECT kind,revision FROM management_resource_catalog WHERE id=$1 AND deleted_at IS NULL FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if current.get::<i64, _>("revision") != expected_revision {
        return Err(ApiError::Conflict(
            "reload the resource before saving".into(),
        ));
    }
    if current.get::<String, _>("kind") != input.kind.text() {
        return Err(invalid("resource kind is immutable"));
    }
    dependencies(&mut tx, id, &input, &ids).await?;
    sqlx::query("UPDATE management_resource_catalog SET name=$2,description=$3,configuration=$4,secret_reference=$5,enabled=$6,revision=revision+1,updated_by=$7,updated_at=clock_timestamp() WHERE id=$1").bind(id).bind(&input.name).bind(&input.description).bind(&input.configuration).bind(&input.secret_reference).bind(input.enabled).bind(principal.user_id).execute(&mut *tx).await?;
    catalog_audit(
        &mut tx,
        principal.user_id,
        id,
        "resource_catalog.updated",
        json!({"previous_revision":expected_revision,"enabled":input.enabled}),
    )
    .await?;
    let value = record(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn remove(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(request): Json<DeleteResource>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    mutation_lock(&mut tx).await?;
    platform_guard(&mut tx, &principal).await?;
    let revision:Option<i64>=sqlx::query_scalar("SELECT revision FROM management_resource_catalog WHERE id=$1 AND deleted_at IS NULL FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    if revision.is_none() {
        return Err(denied());
    }
    if revision != Some(request.expected_revision) {
        return Err(ApiError::Conflict(
            "reload the resource before deleting".into(),
        ));
    }
    let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_resource_dependencies d JOIN management_resource_catalog r ON r.id=d.resource_id WHERE d.depends_on=$1 AND r.deleted_at IS NULL)").bind(id).fetch_one(&mut *tx).await?;
    if used {
        return Err(ApiError::Conflict(
            "remove references from dependent resources before deleting".into(),
        ));
    }
    sqlx::query("UPDATE management_resource_assignment_sets s SET revision=revision+1,updated_by=$2,updated_at=clock_timestamp() WHERE EXISTS(SELECT 1 FROM management_organisations o WHERE o.id=s.organisation_id AND o.archived_at IS NULL) AND EXISTS(SELECT 1 FROM management_resource_assignments a WHERE a.organisation_id=s.organisation_id AND a.user_id=s.user_id AND a.resource_id=$1)").bind(id).bind(principal.user_id).execute(&mut *tx).await?;
    let revoked = sqlx::query("DELETE FROM management_resource_assignments WHERE resource_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM management_resource_dependencies WHERE resource_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE management_resource_catalog SET enabled=FALSE,deleted_at=clock_timestamp(),revision=revision+1,updated_by=$2,updated_at=clock_timestamp() WHERE id=$1").bind(id).bind(principal.user_id).execute(&mut *tx).await?;
    catalog_audit(
        &mut tx,
        principal.user_id,
        id,
        "resource_catalog.deleted",
        json!({"previous_revision":request.expected_revision,"revoked_assignments":revoked}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"deleted":true,"revoked_assignments":revoked}),
    ))
}

#[derive(Clone, Debug, Deserialize, Serialize, Hash, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Assignment {
    pub resource_id: Uuid,
    pub instance_id: Option<Uuid>,
    pub environment_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetAssignments {
    pub expected_revision: i64,
    pub assignments: Vec<Assignment>,
}
async fn member(tx: &mut Transaction<'_, Postgres>, org: Uuid, user: Uuid) -> Result<(), ApiError> {
    let active:Option<Uuid>=sqlx::query_scalar("SELECT m.user_id FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.user_id=$2 AND m.role IN ('admin','member','auditor') AND u.deleted_at IS NULL FOR SHARE OF m,u").bind(org).bind(user).fetch_optional(&mut **tx).await?;
    if active.is_none() {
        return Err(denied());
    }
    Ok(())
}
async fn scope(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
    instance: Uuid,
    environment: Option<Uuid>,
) -> Result<(), ApiError> {
    let allowed:Option<Uuid>=sqlx::query_scalar("SELECT i.id FROM management_instances i JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 WHERE i.organisation_id=$1 AND i.id=$2 AND i.state NOT IN ('deleted','deleting') AND (m.role='admin' OR i.owner_user_id=$3 OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.organisation_id=$1 AND im.instance_id=$2 AND im.user_id=$3)) FOR SHARE OF i,m").bind(org).bind(instance).bind(user).fetch_optional(&mut **tx).await?;
    if allowed.is_none() {
        return Err(denied());
    }
    if let Some(environment) = environment {
        let exists:Option<Uuid>=sqlx::query_scalar("SELECT id FROM management_environments WHERE organisation_id=$1 AND instance_id=$2 AND id=$3 AND state NOT IN ('deleted','deleting') FOR SHARE").bind(org).bind(instance).bind(environment).fetch_optional(&mut **tx).await?;
        if exists.is_none() {
            return Err(denied());
        }
        environment_access::require(
            tx,
            org,
            instance,
            environment,
            user,
            environment_access::Role::Viewer,
        )
        .await?;
    }
    Ok(())
}
async fn assignment_view(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
) -> Result<Value, ApiError> {
    let revision:Option<i64>=sqlx::query_scalar("SELECT revision FROM management_resource_assignment_sets WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).fetch_optional(&mut **tx).await?;
    let assignments:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('resource_id',a.resource_id,'instance_id',a.instance_id,'environment_id',a.environment_id,'available',management_resource_scope_available(a.organisation_id,a.user_id,a.instance_id,a.environment_id,a.resource_id)) FROM management_resource_assignments a WHERE organisation_id=$1 AND user_id=$2 ORDER BY resource_id,instance_id NULLS FIRST,environment_id NULLS FIRST").bind(org).bind(user).fetch_all(&mut **tx).await?;
    Ok(
        json!({"organisation_id":org,"user_id":user,"revision":revision.unwrap_or(0),"assignments":assignments,"delivery":delivery()}),
    )
}
pub(super) async fn assignments(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let admin = org_guard(&mut tx, &principal, org, false).await?;
    if !admin && principal.user_id != user {
        return Err(denied());
    }
    member(&mut tx, org, user).await?;
    let value = assignment_view(&mut tx, org, user).await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn set_assignments(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, user)): Path<(Uuid, Uuid)>,
    Json(request): Json<SetAssignments>,
) -> Result<Json<Value>, ApiError> {
    if request.expected_revision < 0 || request.assignments.len() > 200 {
        return Err(invalid(
            "expected_revision must be nonnegative and at most 200 assignments are allowed",
        ));
    }
    let mut seen = HashSet::new();
    if request.assignments.iter().any(|a| {
        a.resource_id.is_nil()
            || a.environment_id.is_some() && a.instance_id.is_none()
            || !seen.insert(a.clone())
    }) {
        return Err(invalid(
            "assignments need distinct resource/scope tuples; environments require an instance",
        ));
    }
    let mut tx = state.pool.begin().await?;
    mutation_lock(&mut tx).await?;
    org_guard(&mut tx, &principal, org, true).await?;
    member(&mut tx, org, user).await?;
    sqlx::query("INSERT INTO management_resource_assignment_sets(organisation_id,user_id,updated_by) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(org).bind(user).bind(principal.user_id).execute(&mut *tx).await?;
    let revision:i64=sqlx::query_scalar("SELECT revision FROM management_resource_assignment_sets WHERE organisation_id=$1 AND user_id=$2 FOR UPDATE").bind(org).bind(user).fetch_one(&mut *tx).await?;
    if revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "reload the member assignments before saving".into(),
        ));
    }
    for a in &request.assignments {
        let active:Option<Uuid>=sqlx::query_scalar("SELECT id FROM management_resource_catalog WHERE id=$1 AND management_resource_available(id) FOR SHARE").bind(a.resource_id).fetch_optional(&mut *tx).await?;
        if active.is_none() {
            return Err(invalid("assigned resource or dependency is unavailable"));
        }
        if let Some(instance) = a.instance_id {
            scope(&mut tx, org, user, instance, a.environment_id).await?;
        }
    }
    sqlx::query(
        "DELETE FROM management_resource_assignments WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(user)
    .execute(&mut *tx)
    .await?;
    for a in &request.assignments {
        sqlx::query("INSERT INTO management_resource_assignments(id,organisation_id,user_id,resource_id,instance_id,environment_id,assigned_by) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(Uuid::new_v4()).bind(org).bind(user).bind(a.resource_id).bind(a.instance_id).bind(a.environment_id).bind(principal.user_id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE management_resource_assignment_sets SET revision=revision+1,updated_by=$3,updated_at=clock_timestamp() WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).bind(principal.user_id).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "resources.assignments.replaced",
        user,
        json!({"previous_revision":revision,"assignments":request.assignments,"enforced":false}),
    )
    .await?;
    let value = assignment_view(&mut tx, org, user).await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn visible(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let admin = org_guard(&mut tx, &principal, org, false).await?;
    let items:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r)||jsonb_build_object('available',TRUE) FROM management_resource_catalog r WHERE management_resource_available(r.id) AND ($3 OR EXISTS(SELECT 1 FROM management_resource_assignments a WHERE a.organisation_id=$1 AND a.user_id=$2 AND a.resource_id=r.id AND management_resource_scope_available(a.organisation_id,a.user_id,a.instance_id,a.environment_id,a.resource_id))) ORDER BY kind,name,id LIMIT 2000").bind(org).bind(principal.user_id).bind(admin).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"items":items,"delivery":delivery()})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntitlementScope {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
    pub environment_id: Option<Uuid>,
    pub consumer_kind: Option<String>,
    pub consumer_version: Option<u16>,
}
pub(crate) async fn entitlements(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<EntitlementScope>,
) -> Result<Json<Value>, ApiError> {
    let (model_consumer,execution_consumer) = match (q.consumer_kind.as_deref(), q.consumer_version) {
        (None, None) => (false,false),
        (Some("model_provider"), Some(1)) if q.environment_id.is_none() => (true,false),
        (Some("execution_context"), Some(2)) if q.environment_id.is_none() => (false,true),
        _ => {
            return Err(invalid(
                "supported consumers are model_provider v1 and execution_context v2; environment scope requires a bound environment consumer",
            ))
        }
    };
    let mut tx = state.pool.begin().await?;
    crate::fleet::verify_instance_key(&mut tx, &headers, q.organisation_id, q.instance_id).await?;
    member(&mut tx, q.organisation_id, q.user_id).await?;
    scope(
        &mut tx,
        q.organisation_id,
        q.user_id,
        q.instance_id,
        q.environment_id,
    )
    .await?;
    let items:Vec<Value>=sqlx::query_scalar("SELECT DISTINCT to_jsonb(r)||jsonb_build_object('available',TRUE) FROM management_resource_catalog r JOIN management_resource_assignments a ON a.resource_id=r.id WHERE a.organisation_id=$1 AND a.user_id=$2 AND (a.instance_id IS NULL OR a.instance_id=$3) AND (a.environment_id IS NULL OR a.environment_id=$4) AND management_resource_scope_available(a.organisation_id,a.user_id,a.instance_id,a.environment_id,a.resource_id) AND (NOT $5 OR r.kind='model') ORDER BY 1").bind(q.organisation_id).bind(q.user_id).bind(q.instance_id).bind(q.environment_id).bind(model_consumer).fetch_all(&mut *tx).await?;
    // Endpoint configuration is a model dependency, not an independent grant.
    // Never include unrelated endpoints or resolve their secret references here.
    let model_ids: Vec<Uuid> = items
        .iter()
        .filter(|r| r["kind"] == "model")
        .map(|r| serde_json::from_value(r["id"].clone()))
        .collect::<Result<_, _>>()?;
    let dependencies: Vec<Value> = sqlx::query_scalar("SELECT DISTINCT to_jsonb(r)||jsonb_build_object('available',TRUE) FROM management_resource_catalog r JOIN management_resource_dependencies d ON d.depends_on=r.id WHERE d.resource_id=ANY($1) AND r.kind='endpoint' AND management_resource_available(r.id) ORDER BY 1").bind(&model_ids).fetch_all(&mut *tx).await?;
    let revision:Option<i64>=sqlx::query_scalar("SELECT revision FROM management_resource_assignment_sets WHERE organisation_id=$1 AND user_id=$2").bind(q.organisation_id).bind(q.user_id).fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"contract_version":if execution_consumer {2}else{1},"organisation_id":q.organisation_id,"instance_id":q.instance_id,"user_id":q.user_id,"environment_id":q.environment_id,"assignment_revision":revision.unwrap_or(0),"items":items,"dependencies":dependencies,"delivery":delivery(),"enforced":false,
            "consumer":if execution_consumer {json!({"kind":"execution_context","version":2,"enforcement":"revision_bound_dispatch","supported_kinds":["model","mcp","skill","agent_role"],"supported_scopes":["organisation","instance"]})}else if model_consumer {json!({"kind":"model_provider","version":1,"enforcement":"fresh_resolution_only","supported_protocols":["openai_compatible"],"supported_scopes":["organisation","instance"],"immediate_server_runs_only":true})}else{Value::Null}}),
    ))
}

#[cfg(test)]
#[path = "resource_catalog_tests.rs"]
mod tests;
