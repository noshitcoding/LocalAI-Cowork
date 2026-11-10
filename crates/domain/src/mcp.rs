use crate::processes::valid_sha256;
use crate::*;

pub use cowork_contracts::McpServerConfiguration;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpServerRecord {
    pub id: String,
    pub project_id: String,
    pub revision: i64,
    pub configuration: McpServerConfiguration,
    pub configuration_sha256: String,
    pub catalog_sha256: String,
    pub created_at: i64,
    pub updated_at: i64,
}
impl McpServerRecord {
    pub fn public_view(&self) -> McpServer {
        McpServer {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            revision: self.revision,
            configuration: self.configuration.clone(),
            configuration_sha256: self.configuration_sha256.clone(),
            catalog_sha256: self.catalog_sha256.clone(),
        }
    }
    pub fn bindings(&self) -> Result<Vec<McpToolBinding>> {
        if !self.configuration.enabled {
            return Err(Error::Forbidden);
        }
        self.configuration
            .selected_tools
            .iter()
            .map(|name| {
                let definition = self
                    .configuration
                    .catalog
                    .iter()
                    .find(|d| d["name"].as_str() == Some(name))
                    .ok_or(Error::Forbidden)?
                    .clone();
                Ok(McpToolBinding {
                    alias: mcp_alias(&self.id, name),
                    server_id: self.id.clone(),
                    server_revision: self.revision,
                    endpoint: self.configuration.endpoint.clone(),
                    configuration_sha256: self.configuration_sha256.clone(),
                    catalog_sha256: self.catalog_sha256.clone(),
                    definition,
                })
            })
            .collect()
    }
}
pub fn mcp_alias(server: &str, name: &str) -> String {
    format!("mcp_{}", &canonical_hash(&json!([server, name]))[..40])
}
pub fn valid_binding(binding: &McpToolBinding) -> bool {
    let Some(name) = binding.definition.get("name").and_then(Value::as_str) else {
        return false;
    };
    binding.alias == mcp_alias(&binding.server_id, name)
        && !binding.server_id.is_empty()
        && binding.server_id.len() <= 128
        && binding.server_revision > 0
        && valid_sha256(&binding.catalog_sha256)
        && valid_sha256(&binding.configuration_sha256)
        && binding
            .definition
            .get("inputSchema")
            .is_some_and(Value::is_object)
}
pub fn mcp_binding<'a>(run: &'a RunRecord, tool: &ToolCall) -> Result<&'a McpToolBinding> {
    let mut matching = run.spec.mcp_tools.iter().filter(|b| b.alias == tool.name);
    let binding = matching.next().ok_or(Error::Forbidden)?;
    if matching.next().is_some() || !valid_binding(binding) {
        return Err(Error::Forbidden);
    }
    Ok(binding)
}
pub fn mcp_request_hash(run: &RunRecord, tool: &ToolCall) -> String {
    canonical_hash(&json!({"run_id":run.id,"spec":run.spec,"tool":tool}))
}
pub fn transport_request_hash(attempt: &McpAttempt) -> String {
    let mut value = json!({"endpoint":attempt.binding.endpoint,"session":attempt.connection_id,
        "rpc_request_id":attempt.rpc_request_id,"protocol":attempt.transport_protocol.as_deref().unwrap_or(MCP_PROTOCOL_VERSION),
        "catalog_sha256":attempt.binding.catalog_sha256,"name":attempt.binding.definition["name"],"arguments":attempt.tool.arguments});
    if let Some(ttl) = attempt.transport_task_ttl_ms {
        value["task"] = json!({"ttl":ttl});
    }
    canonical_hash(&value)
}
pub fn pending(state: &McpAttemptState) -> bool {
    matches!(
        state,
        McpAttemptState::DispatchCommitted | McpAttemptState::OutcomeUncertain { .. }
    )
}
pub fn validate_result(result: &McpResult) -> Result<()> {
    if !result.response.is_object()
        || serde_json::to_vec(&result.response)
            .map_err(storage_error)?
            .len()
            > 4 * 1024 * 1024
        || result.sha256 != canonical_hash(&result.response)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMcpAttempt {
    pub record: McpAttempt,
    pub lease_sha256: String,
}
impl StoredMcpAttempt {
    pub fn validate(&self) -> Result<()> {
        let a = &self.record;
        for id in [
            &a.id,
            &a.run_id,
            &a.project_id,
            &a.tool.id,
            &a.rpc_request_id,
            &a.connection_id,
        ] {
            valid_text(id, 128)?;
        }
        if !valid_binding(&a.binding)
            || a.transport_protocol.as_deref().is_some_and(|p| {
                !matches!(p, "2025-03-26" | "2025-06-18" | "2024-11-05")
                    || p == MCP_PROTOCOL_VERSION
            })
            || a.transport_task_ttl_ms
                .is_some_and(|t| t == 0 || t > 300_000 || a.transport_protocol.is_some())
            || a.tool.name != a.binding.alias
            || !valid_sha256(&self.lease_sha256)
            || !valid_sha256(&a.request_sha256)
            || !valid_sha256(&a.transport_request_sha256)
            || a.transport_request_sha256 != transport_request_hash(a)
            || a.created_at <= 0
            || a.updated_at < a.created_at
        {
            return Err(Error::Forbidden);
        }
        match &a.state {
            McpAttemptState::DispatchCommitted if a.revision == 1 => {}
            McpAttemptState::OutcomeUncertain {
                code,
                response,
                response_sha256,
            } if a.revision == 2 => {
                valid_text(code, 128)?;
                if !code.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
                    || response.as_ref().map(canonical_hash) != *response_sha256
                    || response.as_ref().is_some_and(|r| {
                        serde_json::to_vec(r).map_or(true, |b| b.len() > 4 * 1024 * 1024)
                    })
                {
                    return Err(Error::Forbidden);
                }
            }
            McpAttemptState::Completed { result } if a.revision == 2 => validate_result(result)?,
            McpAttemptState::ReconciledCompleted {
                result,
                evidence_sha256,
            } if (2..=3).contains(&a.revision) && valid_sha256(evidence_sha256) => {
                validate_result(result)?
            }
            McpAttemptState::ReconciledNotExecuted { evidence_sha256 }
                if (2..=3).contains(&a.revision) && valid_sha256(evidence_sha256) => {}
            _ => return Err(Error::Forbidden),
        }
        Ok(())
    }
    pub fn validate_binding(&self, run: &RunRecord) -> Result<()> {
        self.validate()?;
        if self.record.run_id != run.id
            || self.record.project_id != run.spec.project_id
            || *mcp_binding(run, &self.record.tool)? != self.record.binding
            || mcp_request_hash(run, &self.record.tool) != self.record.request_sha256
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
