//! Independent original-host closure. Local operator trust precedes every new admission.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cowork_contracts::{
    PrepareTeamLostHostRetirement, ReceiveTeamLostHostRetirement, TeamHostTrustProfileView,
    TeamLostHostPlan, TeamLostHostRetirementReceipt, TeamOriginalHostBinding,
};
use sha2::{Digest, Sha256};
#[path = "team_lost_host_recovery/admissions.rs"]
mod admissions;
#[path = "team_lost_host_recovery/crypto.rs"]
mod crypto;
#[path = "team_lost_host_recovery/registry.rs"]
mod registry;
#[path = "team_lost_host_recovery/retirement.rs"]
mod retirement;
fn invalid() -> Error {
    Error::Validation("Invalid original host trust or retirement evidence".into())
}
fn bytes_sha(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(bytes_sha(
        &serde_json::to_vec(value).map_err(storage_error)?,
    ))
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn uuid_value(v: &Value) -> Result<Uuid> {
    let s = v.as_str().ok_or_else(invalid)?;
    let id = Uuid::parse_str(s).map_err(|_| invalid())?;
    if id.is_nil() || id.to_string() != s {
        return Err(invalid());
    }
    Ok(id)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorHostProfile {
    pub format: String,
    pub id: Uuid,
    pub team_id: Uuid,
    pub manager_organisation_id: Uuid,
    pub binding: TeamOriginalHostBinding,
    pub instance_id: Uuid,
    pub vm_id: Uuid,
    pub operator_state_id: Uuid,
    pub execution_epoch: Uuid,
    pub daemon_id: String,
    pub backend_activation: Value,
    pub original_runtime_identity: Value,
    pub original_runtime_signature: Value,
    pub manager_identity_producer_sha256: String,
    pub allocation_base64: String,
    pub allocation_sha256: String,
    pub controller_source_pins: Value,
    pub public_key_der: String,
    pub public_key_sha256: String,
}
impl OperatorHostProfile {
    pub fn validate(&self, observed: &TeamOriginalHostBinding) -> Result<()> {
        if self.format != "localai-team-original-host-trust/1"
            || self.id.is_nil()
            || self.team_id.is_nil()
            || self.manager_organisation_id.is_nil()
            || self.binding != *observed
            || self.binding.format != 1
            || self.binding.host_id.is_nil()
            || self.binding.engine_id.is_empty()
            || self.binding.engine_id.len() > 128
            || self.binding.node_name.is_some() != self.binding.node_uid.is_some()
            || [
                self.instance_id,
                self.vm_id,
                self.operator_state_id,
                self.execution_epoch,
            ]
            .iter()
            .any(Uuid::is_nil)
            || self.daemon_id.len() < 12
            || self.daemon_id.len() > 128
            || self.binding.engine_id != self.daemon_id
        {
            return Err(invalid());
        }
        if self.binding.node_uid.is_some_and(|v| v.is_nil())
            || self
                .binding
                .node_name
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 253)
        {
            return Err(invalid());
        }
        let a = &self.backend_activation;
        let b = &a["observation"]["executor_host_binding"];
        let physical = &a["observation"]["physical"];
        if a["format"] != "cowork-backend-activation-v1"
            || uuid_value(&a["instance_id"])? != self.instance_id
            || uuid_value(&a["organisation_id"])? != self.manager_organisation_id
            || uuid_value(&physical["vm_id"])? != self.vm_id
            || physical["daemon_id"] != self.daemon_id
            || physical["mutual_tls_verified"] != true
            || uuid_value(&b["host_id"])? != self.binding.host_id
            || uuid_value(&b["operator_state_id"])? != self.operator_state_id
        {
            return Err(Error::Forbidden);
        }
        let original = &self.original_runtime_identity;
        if original["format"] != "cowork-manager-host-runtime-identity-v1"
            || uuid_value(&original["organisation_id"])? != self.manager_organisation_id
            || uuid_value(&original["instance_id"])? != self.instance_id
            || uuid_value(&original["host_id"])? != self.binding.host_id
            || uuid_value(&original["vm_id"])? != self.vm_id
            || uuid_value(&original["operator_state_id"])? != self.operator_state_id
            || uuid_value(&original["execution_epoch"])? != self.execution_epoch
            || original["daemon_id"] != self.daemon_id
            || original["executor_host_binding"] != a["observation"]["executor_host_binding"]
            || original["backend_activation_receipt_sha256"] != digest(a)?
            || original["specification_sha256"] != a["specification_sha256"]
            || original["backend_profile_revision"] != a["profile_revision"]
            || original["instance_revision"]
                .as_u64()
                .is_none_or(|v| v == 0)
            || original["backend_profile_revision"]
                .as_u64()
                .is_none_or(|v| v == 0)
            || original["execution_required"] != true
        {
            return Err(Error::Forbidden);
        }
        let signature = &self.original_runtime_signature;
        if signature["format"] != "cowork-manager-host-runtime-identity-signature-v1"
            || signature["identity_sha256"] != digest(original)?
            || signature["public_key_sha256"] != self.public_key_sha256
            || !sha(&self.manager_identity_producer_sha256)
            || signature["producer_source_sha256"] != self.manager_identity_producer_sha256
        {
            return Err(Error::Forbidden);
        }
        crypto::verify_signature(
            self,
            original,
            signature["signature"].as_str().ok_or_else(invalid)?,
        )?;
        let raw = crypto::base64(&self.allocation_base64, 64 * 1024)?;
        let allocation: Value = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        if bytes_sha(&raw) != self.allocation_sha256
            || allocation["adopted"] != false
            || allocation["identity"]["version"] != 2
            || uuid_value(&allocation["identity"]["id"])? != self.vm_id
            || uuid_value(&allocation["identity"]["organisation_id"])?
                != self.manager_organisation_id
            || uuid_value(&allocation["identity"]["operator_state_id"])? != self.operator_state_id
        {
            return Err(Error::Forbidden);
        }
        let fields = [
            "scripts/enterprise-host-retirement-witness.py",
            "deploy/executor-vm/local_executor_vm.py",
            "deploy/executor-vm/capacity_pool.py",
        ];
        if self.controller_source_pins.as_object().is_none_or(|m| {
            m.len() != 3
                || fields
                    .iter()
                    .any(|f| m.get(*f).and_then(Value::as_str).is_none_or(|v| !sha(v)))
        }) {
            return Err(invalid());
        }
        let key = crypto::base64(&self.public_key_der, 2048)?;
        crypto::public_components(&key)?;
        if !sha(&self.public_key_sha256)
            || bytes_sha(&key) != self.public_key_sha256
            || serde_json::to_vec(self).map_err(storage_error)?.len() > 192 * 1024
        {
            return Err(invalid());
        }
        Ok(())
    }
}
/// Only the configured runtime or original pre-Create/current-ID journal provides these bytes.
#[derive(Clone)]
pub struct TeamLostHostRecord {
    pub nonce: Uuid,
    pub raw: Vec<u8>,
    pub original_source: &'static str,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TeamHostAdmissionRef {
    pub profile_id: Uuid,
    pub profile_revision: i64,
    pub profile_sha256: String,
}
/// Opaque closure created from the authenticated immutable SQL/witness tuple.
#[derive(Clone)]
pub struct LostHostClosurePermit {
    service: TeamService,
    actor: Principal,
    plan: TeamLostHostPlan,
    receipt: TeamLostHostRetirementReceipt,
    source: Vec<u8>,
    request_sha256: String,
    operation_run_id: Uuid,
}
impl LostHostClosurePermit {
    pub fn plan(&self) -> &TeamLostHostPlan {
        &self.plan
    }
    pub fn receipt(&self) -> &TeamLostHostRetirementReceipt {
        &self.receipt
    }
    pub fn original_bytes(&self) -> &[u8] {
        &self.source
    }
    pub async fn revalidate(&self) -> Result<()> {
        let mut tx = self
            .service
            .store
            .pool
            .begin()
            .await
            .map_err(storage_error)?;
        self.service
            .container_recovery_authority(&mut tx, &self.actor, self.plan.team_id, self.plan.run_id)
            .await?;
        let row=sqlx::query("SELECT o.request_sha256,m.closure_receipt,m.source_record_sha256 FROM team_lost_host_retirement_operations o JOIN team_lost_host_retirement_members m ON m.operation_run_id=o.run_id AND m.operation_id=o.operation_id WHERE o.run_id=$1 AND o.operation_id=$2 AND m.nonce=$3 AND o.state IN('received','closed')").bind(self.operation_run_id.to_string()).bind(self.receipt.operation_id.to_string()).bind(self.plan.nonce.to_string()).fetch_one(&mut*tx).await.map_err(storage_error)?;
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != self.request_sha256
            || row
                .try_get::<String, _>("closure_receipt")
                .map_err(storage_error)?
                != json_string(&self.receipt)?
            || row
                .try_get::<String, _>("source_record_sha256")
                .map_err(storage_error)?
                != bytes_sha(&self.source)
        {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
}
pub(crate) async fn original_host_files_fence(
    tx: &mut Transaction<'_, Any>,
    order: Uuid,
) -> Result<()> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_container_host_admissions a WHERE a.order_id=$1 AND a.dispatch_admitted=1 AND a.normal_removed_receipt IS NULL AND (a.retired_receipt_sha256 IS NULL OR EXISTS(SELECT 1 FROM team_lost_host_retirement_members m WHERE m.nonce=a.nonce AND m.closed=0))").bind(order.to_string()).fetch_one(&mut**tx).await.map_err(storage_error)?;
    if count != 0 {
        return Err(Error::UncertainEffect);
    }
    Ok(())
}
