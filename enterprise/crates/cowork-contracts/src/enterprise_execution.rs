//! Content-free execution capacity metadata. A reservation has no expiry:
//! loss of connectivity must not grant another executor the same capacity.
use crate::ExecutorTarget;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBudget {
    pub revision: i64,
    pub organisation_concurrent_runs: u32,
    pub user_concurrent_runs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetExecutionBudget {
    pub expected_revision: i64,
    pub organisation_concurrent_runs: u32,
    pub user_concurrent_runs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAdmissionRequest {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub execution_epoch: Uuid,
    pub run_id: Uuid,
    pub user_id: Uuid,
    pub target: ExecutorTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_contract: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionCapacityReason {
    Platform,
    Organisation,
    User,
    Instance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAdmissionDecision {
    pub admitted: bool,
    pub reservation_id: Option<Uuid>,
    pub reason: Option<ExecutionCapacityReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionCompletionOutcome {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReleaseRequest {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub execution_epoch: Uuid,
    pub run_id: Uuid,
    pub reservation_id: Uuid,
    pub completion_id: Uuid,
    pub outcome: ExecutionCompletionOutcome,
    pub execution_stopped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReleaseAcknowledgement {
    pub reservation_id: Uuid,
    pub released: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReservation {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub execution_epoch: Uuid,
    pub run_id: Uuid,
    pub user_id: Uuid,
    pub target: ExecutorTarget,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCapacity {
    pub organisation_id: Uuid,
    pub budget: ExecutionBudget,
    pub held_runs: u64,
    /// Bounded details; held_runs is the untruncated count.
    pub reservations: Vec<ExecutionReservation>,
}

/// Frozen by management when a lifecycle operation is dispatched, never supplied
/// by the user requesting the operation. A candidate is not executable yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGenerationBinding {
    pub operation_id: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub source_epoch: Uuid,
    pub candidate_epoch: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGenerationReceipt {
    pub binding: ExecutionGenerationBinding,
    pub active_epoch: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ExecutionRuntimeAttestation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRuntimeAttestation {
    pub version: u32,
    pub server_image: String,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecoveryReservation {
    pub reservation_id: Uuid,
    pub execution_epoch: Uuid,
    pub run_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecoveryBinding {
    pub generation: ExecutionGenerationBinding,
    pub reservations: Vec<ExecutionRecoveryReservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecoveryReceipt {
    pub binding: ExecutionRecoveryBinding,
    pub stopped: bool,
    pub inventory_sha256: String,
    pub verified_generations: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEnrollmentOutcome {
    Enrolled,
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEnrollmentReceipt {
    pub binding: ExecutionGenerationBinding,
    pub outcome: ExecutionEnrollmentOutcome,
    pub source_manifest_sha256: String,
    pub inventory_sha256: Option<String>,
    pub verified_generations: u32,
}
