#![recursion_limit = "256"]
use cowork_contracts::enterprise::*;
use cowork_contracts::enterprise_execution::*;
use cowork_contracts::organisation_mfa::*;
use serde_json::json;
use uuid::Uuid;

fn main() {
    let id = Uuid::nil();
    let policy = PolicyRevision::default();
    let chat = ChatExecutionPolicy {
        revision: 1,
        environments: [id].into(),
        device_grants: [id].into(),
        transfer_grants: [id].into(),
    };
    let step = ExecutionStep {
        id,
        destination: ExecutionDestination::Environment { id },
        depends_on: Default::default(),
        state: StepState::Pending,
        idempotency_key: id.to_string(),
        required_capabilities: Default::default(),
    };
    let time = "2026-01-01T00:00:00Z".parse().unwrap();
    println!(
        "{}",
        json!({
            "PolicyRevision": policy,
            "SsoGroupMapping": SsoGroupMapping {group:"staff".into(),role:OrganisationRole::Member},
            "OrganisationSsoProvider": OrganisationSsoProvider {id:"primary".into(),label:"Company".into(),issuer:"https://id.example.test".into(),groups_claim:"groups".into()},
            "OrganisationSsoSettings": OrganisationSsoSettings {organisation_id:id,revision:1,enabled:true,available:true,provider_id:Some("primary".into()),auto_provision:true,group_mappings:vec![SsoGroupMapping {group:"staff".into(),role:OrganisationRole::Member}],providers:vec![]},
            "UpdateOrganisationSso": UpdateOrganisationSso {expected_revision:0,enabled:true,provider_id:Some("primary".into()),auto_provision:true,group_mappings:vec![]},
            "OrganisationSsoPublic": OrganisationSsoPublic {enabled:true,organisation_id:id,organisation_name:"Company".into(),provider_label:Some("Company SSO".into())},
            "StartOidcAuthorizationRequest": cowork_contracts::StartOidcAuthorizationRequest {device_id:id,code_challenge:"c".repeat(43),code_challenge_method:"S256".into(),client_state:"s".repeat(43),redirect_uri:"open-cowork://auth/callback".into()},
            "OidcAuthorization": cowork_contracts::OidcAuthorization {schema_version:cowork_contracts::SCHEMA_VERSION,authorization_url:"https://id.example.test/authorize".into(),expires_at:time},
            "AuthenticationAssurance": AuthenticationAssurance { required:true,totp_enabled:true,passkey_registered:false,session_verified:false,enrollment_allowed:false,reauthentication_required:false,provider_verified:Some(false),provider_verification_expires_at:None,oidc_reauthentication_available:Some(false),oidc_organisation_id:None,organisation_id:None,organisation_policy_revision:None,verification_expires_at:None },
            "VerifySecondFactor": cowork_contracts::VerifyTotpRequest { code:"123456".into() },
            "CreateOrganisationInvitation": CreateOrganisationInvitation { email:"invited@example.test".into(),role:OrganisationRole::Member,expires_at:Some(time) },
            "OrganisationInvitation": OrganisationInvitation { id,organisation_id:id,email:"invited@example.test".into(),role:OrganisationRole::Member,invited_by:id,created_at:time,expires_at:time,state:"pending".into() },
            "OrganisationInvitationSecret": OrganisationInvitationSecret { id,organisation_id:id,email:"invited@example.test".into(),role:OrganisationRole::Member,token:"t".repeat(43),expires_at:time },
            "OrganisationInvitationToken": OrganisationInvitationToken { token:"t".repeat(43) },
            "OrganisationInvitationPreview": OrganisationInvitationPreview { id,organisation_id:id,organisation_name:"Invited organisation".into(),email:"invited@example.test".into(),role:OrganisationRole::Member,expires_at:time,account_exists:false },
            "OrganisationInvitationAccepted": OrganisationInvitationAccepted { organisation_id:id,user_id:id,role:OrganisationRole::Member,membership_created:true },
            "RegisterOrganisationInvitation": RegisterOrganisationInvitation { token:"t".repeat(43),display_name:"Invited user".into(),password:"contract-fixture-password".into(),device_id:id },
            "AuthTokens": cowork_contracts::AuthTokens { schema_version:cowork_contracts::SCHEMA_VERSION,access_token:"a".repeat(43),access_expires_at:time,refresh_token:"r".repeat(43),refresh_expires_at:time,user_id:id,session_id:id },
            "InstanceMemberRecord": InstanceMemberRecord { user_id:id, email:"member@example.test".into(), display_name:"Member".into(), role:"viewer".into(), organisation_role:OrganisationRole::Member.into() },
            "InstanceMemberCandidate": SupportAdministrator { user_id:id, email:"member@example.test".into(), display_name:"Member".into() },
            "InstanceBackup": InstanceBackup { id, organisation_id: id, instance_id: id, operation_id: id, sha256: "a".repeat(64), size_bytes: 1024, manifest_version: 1, created_at: time },
            "Instance": Instance { id, organisation_id: id, owner_user_id: id, name: "contract fixture".into(), backend: DeploymentBackend::Compose, state: InstanceState::Pending, policy: policy.clone(), revision: 1, created_at: time, current_release: None, provisioning_release: Some("a".repeat(64)) },
            "CreateInstance": CreateInstance { name:"fixture".into(),backend:DeploymentBackend::Compose,policy:None,release:Some("a".repeat(64)) },
            "ProvisioningRelease": ProvisioningRelease {id:"a".repeat(64),name:"Fixture".into()},
            "Rollout": Rollout { id, organisation_id: id, actor_user_id: id, name: "Fixture".into(), release: "b".repeat(64), batch_size: 1, state: RolloutState::Running, error_code: None, revision: 1, created_at: time, updated_at: time, targets: vec![RolloutTarget { organisation_id: id, rollout_id: id, instance_id: id, instance_name: "Fixture instance".into(), ordinal: 0, source_release: "a".repeat(64), initial_state: InstanceState::Ready, expected_revision: 1, state: RolloutTargetState::Waiting, next_action: Some(InstanceAction::Stop), operation_id: None, error_code: None }] },
            "InstanceContentAccess": InstanceContentAccess { managed:true, organisation_id:Some(id), instance_id:Some(id), policy_revision:Some(1), content_access:Some(ContentAccess::SupportGrant), read_only:true, instance_viewer:false, support_administrator:true, automatic_content_access:false, administrators:vec![SupportAdministrator { user_id:id, display_name:"Support".into(),email:"support@example.test".into() }] },
            "CreateRollout": CreateRollout { name: "Fixture rollout".into(), release: "b".repeat(64), batch_size: 1, targets: vec![RolloutSelection { instance_id: id, expected_revision: 1 }], idempotency_key: id },
            "ControlRollout": ControlRollout { action: RolloutControl::Pause, expected_revision: 1 },
            "InstanceRollback": InstanceRollback { operation_id: id, backup_id: id, source_release: "b".repeat(64), release: "a".repeat(64), created_at: time },
            "InstanceRelease": InstanceRelease { id: "b".repeat(64), name: "Qualified release".into(), source_release: "a".repeat(64), qualified: true },
            "Environment": EnvironmentSpec { instance_id: id, name: "contract fixture".into(), backend: EnvironmentBackend::HyperV, isolation: Isolation::VirtualMachine, template_id: id, template_revision: 1, persistent: true, shared_workspace: false, limits: ResourceLimits::default() },
            "ChatExecutionPolicy": chat,
            "DeviceGrant": DeviceGrant { id, organisation_id: id, instance_id: id, user_id: id, device_id: id, chat_id: id, access: LaptopAccess::None, read_roots: vec![], write_roots: vec![], capabilities: Default::default(), policy_revision: 1, locally_confirmed_at: time, revoked_at: None },
            "TransferGrant": TransferGrant { id, organisation_id: id, instance_id: id, chat_id: id, source: ExecutionDestination::Environment { id }, destination: ExecutionDestination::Device { id, grant_id: id }, relative_paths: vec!["reports/result.docx".into()], writable: true, max_bytes: 1024 },
            "ExecutionStep": step,
            "WorkflowRun": WorkflowRun { id, organisation_id: id, instance_id: id, chat_id: id, policy: chat, steps: vec![step] },
            "ExecutionReleaseAcknowledgement": ExecutionReleaseAcknowledgement {reservation_id:id,released:true},
            "OrganisationMfaPolicy": OrganisationMfaPolicy{organisation_id:id,revision:1,epoch:id,required:false,max_age_seconds:43200,allow_validated_idp:true},
            "UpdateOrganisationMfaPolicy":UpdateOrganisationMfaPolicy{request_id:id,expected_revision:1,required:true,max_age_seconds:600,allow_validated_idp:false,confirmation:"CHANGE_ORGANISATION_MFA".into()},
            "OrganisationMfaReceipt":OrganisationMfaReceipt{request_id:id,actor_user_id:id,policy:OrganisationMfaPolicy{organisation_id:id,revision:2,epoch:id,required:true,max_age_seconds:600,allow_validated_idp:false},previous_revision:1,applied_at:time},
            "InstanceMfaConfiguration":InstanceMfaConfiguration{version:1,instance_id:id,policy:OrganisationMfaPolicy{organisation_id:id,revision:1,epoch:id,required:false,max_age_seconds:43200,allow_validated_idp:true}},
            "ExecutionBudget": ExecutionBudget {revision:0,organisation_concurrent_runs:40,user_concurrent_runs:2},
            "SetExecutionBudget": SetExecutionBudget {expected_revision:0,organisation_concurrent_runs:40,user_concurrent_runs:2},
            "ExecutionAdmissionRequest": ExecutionAdmissionRequest {organisation_id:id,instance_id:id,execution_epoch:id,run_id:id,user_id:id,target:cowork_contracts::ExecutorTarget::ServerLinux {pool_id:None},mfa_contract:None},
            "ExecutionAdmissionDecision": ExecutionAdmissionDecision {admitted:true,reservation_id:Some(id),reason:None},
            "ExecutionReleaseRequest": ExecutionReleaseRequest {organisation_id:id,instance_id:id,execution_epoch:id,run_id:id,reservation_id:id,completion_id:id,outcome:ExecutionCompletionOutcome::Completed,execution_stopped:true},
            "ExecutionReservation": ExecutionReservation {id,organisation_id:id,instance_id:id,execution_epoch:id,run_id:id,user_id:id,target:cowork_contracts::ExecutorTarget::ServerLinux {pool_id:None},created_at:time},
            "ExecutionCapacity": ExecutionCapacity {organisation_id:id,budget:ExecutionBudget {revision:0,organisation_concurrent_runs:40,user_concurrent_runs:2},held_runs:0,reservations:vec![]},
            "ExecutionGenerationBinding": ExecutionGenerationBinding {operation_id:id,organisation_id:id,instance_id:id,source_epoch:id,candidate_epoch:id},
            "ExecutionGenerationReceipt": ExecutionGenerationReceipt {binding:ExecutionGenerationBinding {operation_id:id,organisation_id:id,instance_id:id,source_epoch:id,candidate_epoch:id},active_epoch:id,runtime:Some(ExecutionRuntimeAttestation {version:1,server_image:format!("cowork-server@sha256:{}","a".repeat(64)),manifest_sha256:"b".repeat(64)})},
            "ExecutionEnrollmentReceipt": ExecutionEnrollmentReceipt {binding:ExecutionGenerationBinding {operation_id:id,organisation_id:id,instance_id:id,source_epoch:id,candidate_epoch:id},outcome:ExecutionEnrollmentOutcome::Enrolled,source_manifest_sha256:"a".repeat(64),inventory_sha256:Some("b".repeat(64)),verified_generations:1},
            "ExecutionRuntimeAttestation": ExecutionRuntimeAttestation {version:1,server_image:format!("cowork-server@sha256:{}","a".repeat(64)),manifest_sha256:"b".repeat(64)},
            "ExecutionRecoveryReservation": ExecutionRecoveryReservation {reservation_id:id,execution_epoch:id,run_id:id},
            "ExecutionRecoveryBinding": ExecutionRecoveryBinding {generation:ExecutionGenerationBinding {operation_id:id,organisation_id:id,instance_id:id,source_epoch:id,candidate_epoch:id},reservations:vec![ExecutionRecoveryReservation {reservation_id:id,execution_epoch:id,run_id:id}]},
            "ExecutionRecoveryReceipt": ExecutionRecoveryReceipt {binding:ExecutionRecoveryBinding {generation:ExecutionGenerationBinding {operation_id:id,organisation_id:id,instance_id:id,source_epoch:id,candidate_epoch:id},reservations:vec![ExecutionRecoveryReservation {reservation_id:id,execution_epoch:id,run_id:id}]},stopped:true,inventory_sha256:"a".repeat(64),verified_generations:1},
            "OperationRecoveryRequest": OperationRecoveryRequest { mode: OperationRecoveryMode::ContinueDeletion, confirmation_name:Some("Fixture".into()), purge_backups:true },
            "EnvironmentBackends": [EnvironmentBackend::Docker,EnvironmentBackend::Kubernetes,EnvironmentBackend::Libvirt,EnvironmentBackend::Kubevirt,EnvironmentBackend::HyperV,EnvironmentBackend::OpenShell,EnvironmentBackend::Attached]
        })
    );
}
