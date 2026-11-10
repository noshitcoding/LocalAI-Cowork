//! Explicit organisation MFA policy. Existing organisations remain optional.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganisationMfaPolicy {
    pub organisation_id: Uuid,
    pub revision: i64,
    pub epoch: Uuid,
    pub required: bool,
    pub max_age_seconds: i32,
    pub allow_validated_idp: bool,
}
impl OrganisationMfaPolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.organisation_id.is_nil()
            || self.epoch.is_nil()
            || self.revision < 1
            || !(300..=43_200).contains(&self.max_age_seconds)
        {
            return Err("invalid organisation MFA policy binding or verification age");
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganisationMfaPolicy {
    pub request_id: Uuid,
    pub expected_revision: i64,
    pub required: bool,
    pub max_age_seconds: i32,
    pub allow_validated_idp: bool,
    pub confirmation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganisationMfaReceipt {
    pub request_id: Uuid,
    pub actor_user_id: Uuid,
    pub policy: OrganisationMfaPolicy,
    pub previous_revision: i64,
    pub applied_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceMfaConfiguration {
    pub version: u8,
    pub instance_id: Uuid,
    pub policy: OrganisationMfaPolicy,
}
