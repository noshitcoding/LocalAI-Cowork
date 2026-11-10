//! Team review types staged for the next common plan version.
//! These are not admitted by the existing owner RunRequest or ExecutionPlan.
use cowork_contracts::{CreateTeamRun, TeamDocumentKind, TeamExecutionActor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TeamExecutionKind {
    Team,
}

/// Untrusted intent only. Actor/session/revision facts are resolved by the server
/// and cannot be submitted as part of this request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionRequest {
    pub kind: TeamExecutionKind,
    pub team_id: Uuid,
    pub request: CreateTeamRun,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionModelBinding {
    pub provider_revision: i64,
    pub base_url: String,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TeamExecutionBlocker {
    UnboundExecutionTarget,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionDocumentBinding {
    pub id: Uuid,
    pub kind: TeamDocumentKind,
    pub name: String,
    pub revision: i64,
    pub content_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionMemoryBinding {
    pub id: Uuid,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionMcpBinding {
    pub server_id: Uuid,
    pub revision: i64,
    pub catalog_sha256: String,
    pub tools: Vec<String>,
}

/// A Team preflight must describe the real LiteLLM/team scope. It must not
/// masquerade as an owner Single preflight with native folders or credentials.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionPreflight {
    pub ready: bool,
    pub blockers: Vec<TeamExecutionBlocker>,
    pub actor: TeamExecutionActor,
    pub request: CreateTeamRun,
    pub project_revision: i64,
    pub order_revision: i64,
    pub order_prompt: String,
    pub model: TeamExecutionModelBinding,
    pub documents: Vec<TeamExecutionDocumentBinding>,
    pub personal_knowledge: Vec<TeamExecutionMemoryBinding>,
    pub mcp_servers: Vec<TeamExecutionMcpBinding>,
    pub github_revision: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> serde_json::Value {
        serde_json::json!({"kind":"team","team_id":Uuid::new_v4(),"request":{
            "order_id":Uuid::new_v4(),"prompt":"synthetic review","idempotency_key":"team-review"}})
    }

    #[test]
    fn request_cannot_supply_actor_or_owner_native_authority() {
        let value = intent();
        assert!(serde_json::from_value::<TeamExecutionRequest>(value.clone()).is_ok());
        for field in [
            "actor",
            "session_id",
            "environment",
            "folders",
            "native_tools",
        ] {
            let mut forged = value.clone();
            forged[field] = serde_json::json!("untrusted");
            assert!(
                serde_json::from_value::<TeamExecutionRequest>(forged).is_err(),
                "{field}"
            );
        }
        let mut forged = value;
        forged["request"]["actor"] = serde_json::json!("installation-owner");
        assert!(serde_json::from_value::<TeamExecutionRequest>(forged).is_err());
    }

    #[test]
    fn team_request_does_not_enter_the_current_owner_request_union() {
        assert!(serde_json::from_value::<crate::RunRequest>(intent()).is_err());
    }
}
