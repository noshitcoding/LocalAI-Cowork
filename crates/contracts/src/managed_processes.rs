//! Owner intent for reconciling an existing uncertain managed process. The
//! controller obtains original signed evidence; callers cannot submit a result.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReconcileOwnerManagedProcess {
    pub expected_run_revision: i64,
    pub expected_request_sha256: String,
    /// Close the interrupted run as canceled in the same commit as settlement.
    /// This does not change the original physical process termination evidence.
    pub finish_cancellation: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedProcessReconciliation {
    pub run: RunRecord,
    pub process_attempt: ProcessAttempt,
}
