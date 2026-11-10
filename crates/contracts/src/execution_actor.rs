//! Review provenance, never a credential or a client assertion of authority.
//!
//! The TeamService resolver must construct this from a verified Principal and
//! the identity rows locked in its transaction. Deserializing a review does not
//! authenticate a session. Admission and every later effect must revalidate it.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionActor {
    InstallationOwner {},
    TeamSession { binding: TeamExecutionActor },
}

/// The currently available TeamService content-authority binding. The project
/// resource is scoped by the organization. No invented installation/device epoch
/// is accepted here; the executor/receiver target requires its own plan binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamExecutionActor {
    pub identity_id: Uuid,
    pub session_id: Uuid,
    pub session_revision: i64,
    pub organization_id: Uuid,
    pub organization_epoch: i64,
    pub membership_id: Uuid,
    pub membership_revision: i64,
    pub project_id: Uuid,
    pub resource_revision: i64,
    pub grant_revision: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_label_or_unknown_team_authority_is_not_a_typed_actor() {
        assert!(serde_json::from_str::<ExecutionActor>("\"installation-owner\"").is_err());
        assert!(
            serde_json::from_value::<ExecutionActor>(serde_json::json!({
                "kind":"installation_owner", "organization_id":Uuid::new_v4()
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ExecutionActor>(serde_json::json!({
                "kind":"team_session", "binding":{}
            }))
            .is_err()
        );
    }
}
