//! Snapshot-bound review. The owner route constructs its actor/target from
//! authenticated local authority; neither is submitted by the client.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareOwnerExecutionPlanV2 {
    pub target: OwnerExecutionTargetV2,
    pub request: RunRequest,
    pub input: ExecutionInputRef,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnerExecutionTargetV2 {
    BrokerController {},
    /// Select the operator-registered runner. The controller resolves its current
    /// identity and revision; this selector supplies no runner credentials.
    RegisteredRunner {},
    /// Use the installed bounded document engine on the confirmed local copy.
    /// This is an explicit process destination, not a process-free broker grant.
    DocumentEngine {},
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionDestinationV2 {
    BrokerController {
        controller: BrokerControllerRef,
    },
    DocumentEngine {
        controller: BrokerControllerRef,
        engine_revision: String,
    },
    ManagedRunner {
        controller: BrokerControllerRef,
        target: ExecutionTargetRef,
        runner: ExecutionRunnerIdentity,
        /// Exact package-relative programs in the qualified environment.
        programs: std::collections::BTreeMap<String, String>,
    },
}

impl ExecutionDestinationV2 {
    pub fn controller(&self) -> &BrokerControllerRef {
        match self {
            Self::BrokerController { controller }
            | Self::DocumentEngine { controller, .. }
            | Self::ManagedRunner { controller, .. } => controller,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionPreflightV2 {
    Import {
        review: ImportExecutionPreflightV2,
    },
    Restore {
        review: RestoreExecutionPreflightV2,
    },
    Owner {
        review: RunPreflight,
    },
    Team {
        review: crate::TeamExecutionPreflight,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanV2 {
    pub format_version: u32,
    pub id: String,
    pub actor: ExecutionActor,
    pub project_id: String,
    pub order_id: String,
    pub idempotency_key: String,
    pub request_sha256: String,
    pub spec_sha256: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub target: ExecutionDestinationV2,
    pub input: ExecutionInputSnapshot,
    pub workspace: RunWorkspaceBinding,
    pub preflight: ExecutionPreflightV2,
    pub boundary: ExecutionBoundary,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparedExecutionPlanV2 {
    pub plan: ExecutionPlanV2,
    /// Exact sorted JSON bytes hashed by the custodian. Clients verify these
    /// without reformatting floating-point values or arbitrary input objects.
    pub canonical_json: String,
    pub sha256: String,
    pub committed_run_id: Option<String>,
    pub retired: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_plan_bytes_match_the_independent_client_decimal_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/client/src/fixtures/execution-plan-v2.json"
        ))
        .unwrap();
        let prepared: PreparedExecutionPlanV2 =
            serde_json::from_value(fixture["prepared"].clone()).unwrap();
        let request: PrepareOwnerExecutionPlanV2 =
            serde_json::from_value(fixture["prepare"].clone()).unwrap();
        let run: RunRecord = serde_json::from_value(fixture["run"].clone()).unwrap();
        assert!(prepared.canonical_json.contains("\"temperature\":0.0"));
        assert_eq!(
            cowork_domain::hash(prepared.canonical_json.as_bytes()),
            prepared.sha256
        );
        assert_eq!(
            cowork_domain::canonical_hash(&prepared.plan),
            prepared.sha256
        );
        assert_eq!(
            cowork_domain::canonical_hash(&request),
            prepared.plan.request_sha256
        );
        assert_eq!(
            cowork_domain::canonical_hash(&run.spec),
            prepared.plan.spec_sha256
        );
    }
}
