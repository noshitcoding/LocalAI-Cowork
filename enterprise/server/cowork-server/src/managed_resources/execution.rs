//! Revision-bound central skills, roles and MCP. Catalog grants never grant tools.
use super::*;
use crate::{auth::Principal, mcp_bindings::ResolvedServerMcpBinding};
use axum::{extract::Extension, Json};
use cowork_contracts::ExecutorTarget;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct ExecutionManifest {
    contract_version: u16,
    organisation_id: Uuid,
    instance_id: Uuid,
    user_id: Uuid,
    environment_id: Option<Uuid>,
    assignment_revision: i64,
    items: Vec<Resource>,
    consumer: Consumer,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u16,
    organisation_id: Uuid,
    instance_id: Uuid,
    user_id: Uuid,
    assignment_revision: i64,
    resources: Vec<Revision>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    id: Uuid,
    revision: i64,
    kind: String,
}
fn invalid(message: &str) -> ApiError {
    ApiError::Conflict(message.to_owned())
}
fn unavailable() -> ApiError {
    ApiError::Unavailable(
        "central resource authority or target-local MCP binding is unavailable".into(),
    )
}
async fn manifest(config: &Configuration, user: Uuid) -> Result<ExecutionManifest, ApiError> {
    let client = crate::management_transport::client(&config.base, Duration::from_secs(5))?;
    let mut response = client
        .get(format!(
            "{}/management/runner/v1/resource-entitlements",
            config.base.trim_end_matches('/')
        ))
        .header("x-cowork-instance-key", &config.key)
        .query(&[
            ("organisation_id", config.organisation.to_string()),
            ("instance_id", config.instance.to_string()),
            ("user_id", user.to_string()),
            ("consumer_kind", "execution_context".to_owned()),
            ("consumer_version", "2".to_owned()),
        ])
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    let result: ExecutionManifest = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if result.contract_version != 2
        || result.organisation_id != config.organisation
        || result.instance_id != config.instance
        || result.user_id != user
        || result.environment_id.is_some()
        || result.assignment_revision < 0
        || result.items.len() > 200
        || result.consumer.kind != "execution_context"
        || result.consumer.version != 2
    {
        return Err(unavailable());
    }
    let mut ids = HashSet::new();
    for item in &result.items {
        if item.id.is_nil()
            || item.revision < 1
            || !item.enabled
            || !item.available
            || item.deleted_at.is_some()
            || !ids.insert(item.id)
            || !matches!(
                item.kind.as_str(),
                "model" | "mcp" | "skill" | "agent_role" | "endpoint"
            )
        {
            return Err(unavailable());
        }
    }
    Ok(result)
}
pub(crate) async fn list(
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let Some(config) = configuration()? else {
        return Ok(Json(json!([])));
    };
    let result = manifest(&config, principal.user_id).await?;
    Ok(Json(Value::Array(result.items.iter().filter(|item|matches!(item.kind.as_str(),"skill"|"agent_role"|"mcp")).map(|item|{
        let resolved=resolve(&result,&[item.id]);
        let needs_mcp=resolved.as_ref().is_ok_and(|items|items.iter().any(|r|r.kind=="mcp"));
        json!({"id":item.id,"revision":item.revision,"kind":item.kind,"name":item.name,"available":resolved.is_ok(),"required_capabilities":if needs_mcp {vec!["tool.mcp.invoke"]}else{vec![]}})
    }).collect())))
}
fn selected(input: &Value) -> Result<Vec<Uuid>, ApiError> {
    let Some(value) = input.get("managed_resource_ids") else {
        return Ok(vec![]);
    };
    let ids: Vec<Uuid> = serde_json::from_value(value.clone())
        .map_err(|_| invalid("managed_resource_ids must contain resource UUIDs"))?;
    if ids.len() > 64
        || ids.iter().any(Uuid::is_nil)
        || ids.iter().collect::<HashSet<_>>().len() != ids.len()
    {
        return Err(invalid(
            "managed resource selection must be distinct and bounded",
        ));
    }
    Ok(ids)
}
fn resolve<'a>(
    manifest: &'a ExecutionManifest,
    ids: &[Uuid],
) -> Result<Vec<&'a Resource>, ApiError> {
    let mut pending = ids.to_vec();
    let mut seen = BTreeSet::new();
    let mut result = vec![];
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if seen.len() > 100 {
            return Err(invalid(
                "managed resource dependencies exceed the execution limit",
            ));
        }
        let resource = manifest.items.iter().find(|r| r.id == id).ok_or_else(|| {
            invalid("a selected managed resource or role dependency is no longer assigned")
        })?;
        if resource.kind == "agent_role" {
            let dependencies: Vec<Uuid> = serde_json::from_value(
                resource
                    .configuration
                    .get("resource_ids")
                    .cloned()
                    .unwrap_or(json!([])),
            )?;
            pending.extend(dependencies);
        }
        if resource.kind == "mcp" {
            resolve_mcp_secret(resource.secret_reference.as_deref())?;
        }
        result.push(resource);
    }
    result.sort_by_key(|r| r.id);
    if result.iter().filter(|r| r.kind == "agent_role").count() > 1 {
        return Err(invalid("select one central agent role per run"));
    }
    Ok(result)
}
pub(crate) async fn freeze(
    user: Uuid,
    mut input: Value,
    target: &ExecutorTarget,
    model: &mut Option<Uuid>,
) -> Result<Value, ApiError> {
    if let Some(object) = input.as_object_mut() {
        object.remove("_managed_resource_binding");
    }
    let ids = selected(&input)?;
    if ids.is_empty() {
        return Ok(input);
    }
    if !matches!(target, ExecutorTarget::ServerLinux { .. })
        || input.get("crew_definition").is_some()
    {
        return Err(invalid(
            "central execution resources require the upgraded server agent consumer",
        ));
    }
    let config = configuration()?.ok_or_else(unavailable)?;
    let manifest = manifest(&config, user).await?;
    let resources = resolve(&manifest, &ids)?;
    let models: Vec<_> = resources.iter().filter(|r| r.kind == "model").collect();
    if models.len() > 1
        || models
            .first()
            .is_some_and(|r| model.is_some_and(|id| id != r.id))
    {
        return Err(invalid(
            "the selected model conflicts with the central role model",
        ));
    }
    if let Some(resource) = models.first() {
        *model = Some(resource.id);
    }
    let object = input
        .as_object_mut()
        .ok_or_else(|| invalid("managed execution requires object input"))?;
    let context = object
        .entry("frozen_runtime_context")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| invalid("invalid frozen context"))?;
    let skills = context
        .entry("skill")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| invalid("invalid frozen skills"))?;
    for resource in &resources {
        if resource.kind == "skill" {
            skills.push(json!({"entity_id":resource.id,"revision":resource.revision,"definition":{"name":resource.name,"prompt_template":resource.configuration["instructions"],"source":"management_catalog"}}));
        }
    }
    if let Some(role) = resources.iter().find(|r| r.kind == "agent_role") {
        let prompt = role.configuration["system_prompt"]
            .as_str()
            .ok_or_else(unavailable)?;
        let base=object.get("system_prompt").and_then(Value::as_str).unwrap_or("You are Open Cowork. Complete the task using only approved tools and assigned workspace files.");
        object.insert(
            "system_prompt".into(),
            json!(format!(
                "{base}\n\nAssigned central agent role ({}):\n{prompt}",
                role.name
            )),
        );
    }
    let binding = Binding {
        version: 2,
        organisation_id: config.organisation,
        instance_id: config.instance,
        user_id: user,
        assignment_revision: manifest.assignment_revision,
        resources: resources
            .iter()
            .map(|r| Revision {
                id: r.id,
                revision: r.revision,
                kind: r.kind.clone(),
            })
            .collect(),
    };
    object.insert(
        "_managed_resource_binding".into(),
        serde_json::to_value(binding)?,
    );
    if serde_json::to_vec(&input)?.len() > 256 * 1024 {
        return Err(invalid("managed runtime context exceeds 256 KiB"));
    }
    Ok(input)
}
async fn current(input: &Value, user: Uuid) -> Result<Vec<Resource>, ApiError> {
    let Some(value) = input.get("_managed_resource_binding") else {
        return Ok(vec![]);
    };
    let binding: Binding = serde_json::from_value(value.clone()).map_err(|_| unavailable())?;
    let config = configuration()?.ok_or_else(unavailable)?;
    let result = manifest(&config, user).await?;
    verify_binding(&binding, &config, user, &result)
}
fn verify_binding(
    binding: &Binding,
    config: &Configuration,
    user: Uuid,
    result: &ExecutionManifest,
) -> Result<Vec<Resource>, ApiError> {
    if binding.version != 2
        || binding.user_id != user
        || binding.organisation_id != config.organisation
        || binding.instance_id != config.instance
        || binding.resources.len() > 100
    {
        return Err(unavailable());
    }
    if result.assignment_revision != binding.assignment_revision {
        return Err(invalid("central assignments changed; prepare a new run"));
    }
    binding
        .resources
        .iter()
        .map(|expected| {
            result
                .items
                .iter()
                .find(|r| {
                    r.id == expected.id
                        && r.revision == expected.revision
                        && r.kind == expected.kind
                })
                .cloned()
                .ok_or_else(|| {
                    invalid("a central resource changed or was revoked; prepare a new run")
                })
        })
        .collect()
}
pub(crate) async fn authorize(input: &Value, user: Uuid) -> Result<(), ApiError> {
    current(input, user).await.map(|_| ())
}
pub(crate) async fn mcp(
    input: &Value,
    user: Uuid,
) -> Result<Vec<ResolvedServerMcpBinding>, ApiError> {
    let resources = current(input, user).await?;
    let mut result = vec![];
    for resource in resources.into_iter().filter(|r| r.kind == "mcp") {
        let config = &resource.configuration;
        let transport = config["transport"]
            .as_str()
            .ok_or_else(unavailable)?
            .to_owned();
        let mut environment = BTreeMap::new();
        let mut headers = BTreeMap::new();
        if let Some(secret) = resolve_mcp_secret(resource.secret_reference.as_deref())? {
            if transport == "stdio" {
                environment.insert("COWORK_MCP_TOKEN".into(), secret);
            } else {
                headers.insert("Authorization".into(), format!("Bearer {secret}"));
            }
        }
        result.push(ResolvedServerMcpBinding {
            name: format!("managed_{}", resource.id.simple()),
            transport,
            command: config["command"].as_str().unwrap_or("").to_owned(),
            args: serde_json::from_value(config.get("args").cloned().unwrap_or(json!([])))?,
            environment,
            url: config["url"].as_str().unwrap_or("").to_owned(),
            headers,
        });
    }
    Ok(result)
}

fn resolve_mcp_secret(reference: Option<&str>) -> Result<Option<String>, ApiError> {
    // Target-local MCP credentials have their own namespace. Catalog entries
    // must never read provider, authentication or arbitrary process secrets.
    resolve_resource_secret(
        reference,
        "COWORK_MANAGED_MCP_SECRET_",
        "/run/secrets/cowork-managed-mcp",
    )
    .map_err(|_| unavailable())
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
