//! Shared Team review/effect freshness. Public review data is never a credential.
use super::*;
use cowork_contracts::TeamExecutionActor;

fn actor(permit: &ContentPermit) -> TeamExecutionActor {
    let p = &permit.0;
    TeamExecutionActor {
        identity_id: p.identity_id,
        session_id: p.session_id,
        session_revision: p.session_revision,
        organization_id: p.organization_id,
        organization_epoch: p.authorization_epoch,
        membership_id: p.membership_id,
        membership_revision: p.membership_revision,
        project_id: p.resource_id,
        resource_revision: p.resource_revision,
        grant_revision: p.grant_revision,
    }
}

fn retained_permit(binding: &TeamExecutionActor, action: ContentAction) -> ContentPermit {
    ContentPermit(PermitData {
        identity_id: binding.identity_id,
        session_id: binding.session_id,
        session_revision: binding.session_revision,
        organization_id: binding.organization_id,
        authorization_epoch: binding.organization_epoch,
        membership_id: binding.membership_id,
        membership_revision: binding.membership_revision,
        resource_id: binding.project_id,
        resource_revision: binding.resource_revision,
        grant_revision: binding.grant_revision,
        action,
    })
}

impl IdentityStore {
    /// Validate original global session facts loaded from a retained personal run.
    /// The caller authenticates access to that stored run, preserves its separate
    /// scheduled-origin authority and holds this writer transaction through the
    /// effect. This is not sign-in of submitted IDs and returns no Principal,
    /// permit or IO authority. Product ownership and lease gates stay separate.
    pub async fn validate_retained_personal_device_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        original_session: (Uuid, Uuid, i64),
    ) -> Result<()> {
        let (identity_id, session_id, session_revision) = original_session;
        let principal = Principal {
            identity_id,
            session_id,
            session_revision,
        };
        self.require_global_session_in_transaction(tx, &principal)
            .await
    }

    /// Recheck original session facts loaded from a retained personal server run.
    /// The caller already holds its product writer and original lease checks;
    /// scheduled-origin authority is handled separately. Returns no Principal,
    /// permit or IO authority and does not replace the product configuration.
    pub async fn validate_retained_personal_server_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        original_session: (Uuid, Uuid, i64),
    ) -> Result<()> {
        let (identity_id, session_id, session_revision) = original_session;
        let principal = Principal {
            identity_id,
            session_id,
            session_revision,
        };
        self.require_global_session_in_transaction(tx, &principal)
            .await
    }

    /// Revalidate original session facts loaded from a retained Team device run
    /// or desktop grant. This is not authentication of submitted IDs and returns
    /// no Principal, permit or IO authority. The caller authenticates access to
    /// that stored record and keeps its writer transaction through the effect.
    /// Creator/control paths retain Execute; viewing retains Read. Original
    /// epoch/project, target, snapshot, credential and lease gates stay separate.
    pub async fn validate_retained_team_device_actor_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        original_session: (Uuid, Uuid, i64),
        organization: Uuid,
        project: Uuid,
        action: ContentAction,
    ) -> Result<()> {
        let (identity_id, session_id, session_revision) = original_session;
        let principal = Principal {
            identity_id,
            session_id,
            session_revision,
        };
        let permit = self
            .authorize_content_in_transaction(tx, &principal, organization, project, action)
            .await?;
        self.validate_content_in_transaction(tx, &permit).await
    }

    /// Validate the session facts read from an already admitted product approval.
    /// Approvals retain the exact approving session, while its content rights are
    /// checked afresh. This is not a sign-in API and returns no authority token.
    pub async fn validate_retained_team_approver_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        identity: Uuid,
        session: Uuid,
        session_revision: i64,
        organization: Uuid,
        project: Uuid,
    ) -> Result<()> {
        let principal = Principal {
            identity_id: identity,
            session_id: session,
            session_revision,
        };
        self.resolve_team_execution_actor_in_transaction(tx, &principal, organization, project)
            .await?;
        Ok(())
    }

    /// Requires a genuinely authenticated Principal. Resolve under the identity
    /// writer before product locks; no client-supplied actor is authenticated here.
    pub async fn resolve_team_execution_actor_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        principal: &Principal,
        organization: Uuid,
        project: Uuid,
    ) -> Result<TeamExecutionActor> {
        let permit = self
            .authorize_content_in_transaction(
                tx,
                principal,
                organization,
                project,
                ContentAction::Execute,
            )
            .await?;
        let binding = actor(&permit);
        self.validate_retained_team_execution_actor_in_transaction(tx, &binding)
            .await?;
        Ok(binding)
    }

    /// Recheck facts loaded from a trusted retained plan/run, after authenticating
    /// access to that product record. This does NOT authenticate a caller, mint a
    /// ContentPermit or authorize IO. Never use a submitted review as a credential.
    /// Keep the writer transaction through the associated durable product effect;
    /// model/tool/target/input/budget and lease gates remain separately required.
    pub async fn validate_retained_team_execution_actor_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        binding: &TeamExecutionActor,
    ) -> Result<()> {
        for action in [ContentAction::Read, ContentAction::Execute] {
            self.validate_content_in_transaction(tx, &retained_permit(binding, action))
                .await?;
        }
        Ok(())
    }
}
