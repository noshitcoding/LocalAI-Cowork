//! Durable, original-owner OAuth authority. An exchange receipt never grants a
//! second token POST, and an unknown refresh makes the old grant unusable.
use super::*;
use cowork_identity::{ContentAction, Principal};
use cowork_mcp::oauth::{AuthorizationMetadata, ProtectedResourceMetadata};
use cowork_secrets::{
    KeyId, KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob, seal_mcp_blob,
};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction, any::AnyRow};
use zeroize::{Zeroize, Zeroizing};

pub enum McpOAuthOwner<'a> {
    Native {
        key: KeyId,
    },
    Personal {
        actor: &'a Principal,
        key: KeyId,
    },
    Team {
        actor: &'a Principal,
        team: Uuid,
        key: KeyId,
    },
    TeamExecution {
        authority: &'a crate::team_service::TeamExecutionAuthority,
        key: KeyId,
    },
    PersonalExecution {
        owner: Uuid,
        run: Uuid,
        lease: Uuid,
        key: KeyId,
    },
}
impl McpOAuthOwner<'_> {
    pub fn key(&self) -> KeyId {
        match self {
            Self::Native { key }
            | Self::Personal { key, .. }
            | Self::Team { key, .. }
            | Self::TeamExecution { key, .. }
            | Self::PersonalExecution { key, .. } => *key,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Native { .. } => "native",
            Self::Personal { .. } => "personal",
            Self::Team { .. } => "team",
            Self::TeamExecution { .. } => "team",
            Self::PersonalExecution { .. } => "personal",
        }
    }
    pub fn id(&self) -> String {
        match self {
            Self::Native { key } => key.instance.to_string(),
            Self::Personal { actor, .. } => actor.identity_id().to_string(),
            Self::Team { team, .. } => team.to_string(),
            Self::TeamExecution { authority, .. } => authority.team_id().to_string(),
            Self::PersonalExecution { owner, .. } => owner.to_string(),
        }
    }
    fn actor(&self) -> Value {
        match self {
            Self::Native { key } => json!({"native_instance":key.instance}),
            Self::Personal { actor, .. } | Self::Team { actor, .. } => {
                json!({"identity_id":actor.identity_id(),"session_id":actor.session_id()})
            }
            Self::TeamExecution { authority, .. } => {
                json!({"run_id":authority.run_id(),"identity_id":authority.identity_id(),"session_id":authority.session_id()})
            }
            Self::PersonalExecution {
                run, owner, lease, ..
            } => json!({"run_id":run,"identity_id":owner,"lease_id":lease}),
        }
    }
    fn management(&self) -> bool {
        matches!(
            self,
            Self::Native { .. } | Self::Personal { .. } | Self::Team { .. }
        )
    }
}
async fn original_actor(tx:&mut Transaction<'_,Any>,owner:&McpOAuthOwner<'_>)->Result<Value>{
    let mut actor=owner.actor();
    if let McpOAuthOwner::Personal{actor:principal,..}|McpOAuthOwner::Team{actor:principal,..}=owner {
        let revision:i64=sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(principal.session_id().to_string()).bind(principal.identity_id().to_string()).bind(now()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        actor["session_revision"]=json!(revision);
    }Ok(actor)
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthBinding {
    pub id: String,
    pub revision: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthProfileRecord {
    pub view: McpOAuthProfile,
    pub epoch: Uuid,
    pub authorization: AuthorizationMetadata,
    pub resource_metadata: ProtectedResourceMetadata,
    pub client_scope: McpSecretScope,
    pub sealed_client: Option<SealedMcpBlob>,
    pub active_token_id: Option<String>,
    pub pending_exchange_id: Option<String>,
}
impl McpOAuthProfileRecord {
    pub fn reference(&self) -> String {
        format!("oauth:{}", self.view.id)
    }
    fn scope(&self, id: &str, revision: i64, purpose: &str) -> McpSecretScope {
        McpSecretScope {
            owner_kind: self.view.owner_kind.clone(),
            owner_id: self.view.owner_id.clone(),
            project_id: self.view.project_id.clone(),
            server_id: self.view.server_id.clone(),
            record_id: id.into(),
            revision,
            epoch: self.epoch,
            configuration_sha256: self.view.binding_sha256.clone(),
            purpose: purpose.into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthFlowSecret {
    pub state: String,
    pub verifier: String,
    pub authorization_url: String,
    pub expires_at: i64,
    pub actor: Value,
}
impl Drop for McpOAuthFlowSecret {
    fn drop(&mut self) {
        self.state.zeroize();
        self.verifier.zeroize();
        self.authorization_url.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthCallback {
    pub state: String,
    pub code: Option<String>,
    pub error: Option<String>,
    #[serde(default, rename = "iss")]
    pub issuer: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}
impl Drop for McpOAuthCallback {
    fn drop(&mut self) {
        self.state.zeroize();
        if let Some(code) = &mut self.code {
            code.zeroize();
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthFlowRecord {
    pub view: McpOAuthFlow,
    pub profile_epoch: Uuid,
    pub binding_sha256: String,
    pub actor: Value,
    pub scope: McpSecretScope,
    pub sealed: SealedMcpBlob,
    pub callback: Option<SealedMcpBlob>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthTokenSecret {
    pub access: String,
    pub refresh: Option<String>,
}
impl Drop for McpOAuthTokenSecret {
    fn drop(&mut self) {
        self.access.zeroize();
        if let Some(v) = &mut self.refresh {
            v.zeroize();
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthTokenRecord {
    pub id: String,
    pub profile_id: String,
    pub revision: i64,
    pub grant_id: String,
    pub state: String,
    pub expires_at: i64,
    pub scopes: Vec<String>,
    pub scope: McpSecretScope,
    pub sealed: SealedMcpBlob,
}
pub struct McpOAuthExchangePermit {
    pub id: String,
    pub profile: McpOAuthProfileRecord,
    pub source_id: String,
    pub kind: String,
    pub request_sha256: String,
    pub authority_sha256: String,
    pub scope: McpSecretScope,
}
pub enum McpOAuthExchangeAdmission {
    Dispatch(McpOAuthExchangePermit),
    Original(Value),
}

fn invalid() -> Error {
    Error::Validation("Invalid MCP OAuth configuration or original authority".into())
}
fn sha(raw: &str) -> String {
    canonical_hash(&raw)
}
fn profile_row(r: &AnyRow) -> Result<McpOAuthProfileRecord> {
    let view: McpOAuthProfile = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if view.id != r.try_get::<String, _>("id").map_err(storage_error)?
        || view.revision != r.try_get::<i64, _>("revision").map_err(storage_error)?
        || view.owner_kind
            != r.try_get::<String, _>("owner_kind")
                .map_err(storage_error)?
        || view.owner_id != r.try_get::<String, _>("owner_id").map_err(storage_error)?
        || view.project_id
            != r.try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || view.server_id != r.try_get::<String, _>("server_id").map_err(storage_error)?
        || view.binding_sha256
            != r.try_get::<String, _>("binding_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    // Exact original metadata accompanies the encrypted client binding; it is
    // not reconstructed by querying a subsequently changed provider.
    let metadata: Value = parse(
        &r.try_get::<String, _>("secret_scope")
            .map_err(storage_error)?,
    )?;
    let client_scope: McpSecretScope =
        serde_json::from_value(metadata["scope"].clone()).map_err(storage_error)?;
    client_scope.validate().map_err(|_| Error::Forbidden)?;
    let epoch = Uuid::parse_str(&r.try_get::<String, _>("epoch").map_err(storage_error)?)
        .map_err(|_| Error::Forbidden)?;
    let p = McpOAuthProfileRecord {
        view,
        epoch,
        authorization: serde_json::from_value(metadata["authorization"].clone())
            .map_err(storage_error)?,
        resource_metadata: serde_json::from_value(metadata["resource"].clone())
            .map_err(storage_error)?,
        client_scope,
        sealed_client: r
            .try_get::<Option<String>, _>("sealed_client")
            .map_err(storage_error)?
            .map(|v| parse(&v))
            .transpose()?,
        active_token_id: r.try_get("active_token_id").map_err(storage_error)?,
        pending_exchange_id: r.try_get("pending_exchange_id").map_err(storage_error)?,
    };
    let binding = canonical_hash(&json!([
        1,
        p.view.owner_kind,
        p.view.owner_id,
        p.view.project_id,
        p.view.server_id,
        p.view.revision,
        p.epoch,
        p.view.configuration,
        p.resource_metadata.sha256
    ]));
    let resource_sha = canonical_hash(&json!([
        1,
        p.resource_metadata.resource,
        p.resource_metadata.metadata_uri,
        p.resource_metadata.original,
        p.resource_metadata.authorization_servers
    ]));
    if binding != p.view.binding_sha256
        || resource_sha != p.resource_metadata.sha256
        || canonical_hash(&p.authorization.original) != p.authorization.sha256
        || p.client_scope != p.scope(&p.view.id, p.view.revision, "oauth_profile")
        || p.epoch.is_nil()
        || p.authorization.issuer != p.view.configuration.authorization_server
        || p.resource_metadata.sha256 != p.view.metadata_sha256
        || p.resource_metadata.resource != p.view.configuration.resource
        || !p
            .resource_metadata
            .authorization_servers
            .contains(&p.authorization)
    {
        return Err(Error::Forbidden);
    }
    Ok(p)
}
async fn lock_profile(tx: &mut Transaction<'_, Any>, id: &str) -> Result<McpOAuthProfileRecord> {
    if sqlx::query("UPDATE mcp_oauth_profiles SET revision=revision WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    profile_row(
        &sqlx::query("SELECT * FROM mcp_oauth_profiles WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )
}
impl Store {
    pub async fn mcp_oauth_configuration_receipt(&self,owner:&McpOAuthOwner<'_>,project:&str,input:&ConfigureMcpOAuth,keys:&dyn KeyStore)->Result<Option<McpOAuthProfile>>{
        if !owner.management(){return Err(Error::Forbidden);}let mut tx=self.pool.begin().await.map_err(storage_error)?;self.mcp_oauth_authority(&mut tx,owner,project,&input.server_id,None,Some(&input.configuration.resource),keys).await?;
        let original=sqlx::query("SELECT request_sha256,request_key_id,receipt FROM mcp_oauth_configuration_operations WHERE owner_kind=$1 AND owner_id=$2 AND project_id=$3 AND request_key=$4").bind(owner.kind()).bind(owner.id()).bind(project).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let result=if let Some(row)=original{let id:KeyId=parse(&row.try_get::<String,_>("request_key_id").map_err(storage_error)?)?;let material=keys.load(&id).map_err(|_|Error::NativeKeyUnavailable)?;let bytes=Zeroizing::new(serde_json::to_vec(&json!(["mcp-oauth-config-request/v2",owner.kind(),owner.id(),project,input])).map_err(storage_error)?);let tag=ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256,material.as_ref()),&bytes);let digest=tag.as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();if digest!=row.try_get::<String,_>("request_sha256").map_err(storage_error)?{return Err(Error::Conflict);}Some(parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?)}else{None};tx.commit().await.map_err(storage_error)?;Ok(result)
    }
    pub async fn discover_scoped_mcp_oauth(&self,owner:&McpOAuthOwner<'_>,project:&str,input:&McpOAuthDiscovery,keys:&dyn KeyStore)->Result<McpOAuthMetadata>{
        if !owner.management()||owner.kind()=="native"{return Err(Error::Forbidden);}let mut tx=self.pool.begin().await.map_err(storage_error)?;
        let before=self.mcp_oauth_authority(&mut tx,owner,project,&input.server_id,Some(input.server_revision),Some(&input.resource),keys).await?;tx.commit().await.map_err(storage_error)?;
        let metadata=cowork_mcp::oauth::discover(&input.resource,cowork_mcp::oauth::OAuthNetwork::PublicServer).await.map_err(|_|Error::McpSessionUnavailable)?;
        let mut tx=self.pool.begin().await.map_err(storage_error)?;let after=self.mcp_oauth_authority(&mut tx,owner,project,&input.server_id,Some(input.server_revision),Some(&input.resource),keys).await?;if before!=after{return Err(Error::Forbidden);}tx.commit().await.map_err(storage_error)?;
        Ok(McpOAuthMetadata{resource:metadata.resource,resource_metadata_uri:metadata.metadata_uri,authorization_servers:metadata.authorization_servers.into_iter().map(|m|json!({"issuer":m.issuer,"authorization_endpoint":m.authorization_endpoint,"token_endpoint":m.token_endpoint,"scopes_supported":m.scopes_supported,"token_endpoint_auth_methods_supported":m.token_endpoint_auth_methods_supported})).collect(),scopes_supported:metadata.scopes_supported,suggested_scopes:metadata.suggested_scopes,sha256:metadata.sha256})
    }
    pub async fn configure_scoped_mcp_oauth(&self,owner:&McpOAuthOwner<'_>,project:&str,input:&ConfigureMcpOAuth,expected_redirect:&str,keys:&dyn KeyStore)->Result<McpOAuthProfile>{
        if !owner.management()||owner.kind()=="native"||input.configuration.redirect_uri!=expected_redirect{return Err(Error::Forbidden);}if let Some(receipt)=self.mcp_oauth_configuration_receipt(owner,project,input,keys).await?{return Ok(receipt);}let mut tx=self.pool.begin().await.map_err(storage_error)?;
        let before=self.mcp_oauth_authority(&mut tx,owner,project,&input.server_id,Some(input.server_revision),Some(&input.configuration.resource),keys).await?;tx.commit().await.map_err(storage_error)?;
        let metadata=cowork_mcp::oauth::discover(&input.configuration.resource,cowork_mcp::oauth::OAuthNetwork::PublicServer).await.map_err(|_|Error::McpSessionUnavailable)?;
        let mut tx=self.pool.begin().await.map_err(storage_error)?;let after=self.mcp_oauth_authority(&mut tx,owner,project,&input.server_id,Some(input.server_revision),Some(&input.configuration.resource),keys).await?;if before!=after{return Err(Error::Forbidden);}tx.commit().await.map_err(storage_error)?;
        self.configure_mcp_oauth(owner,project,input,&metadata,keys).await
    }
    /// Public browser ingress records only the exact encrypted original flow.
    /// A current authenticated management session must separately adopt the
    /// response. No Principal or token-POST permit is reconstructed here.
    pub async fn capture_server_mcp_oauth_callback(&self,callback:&McpOAuthCallback,redirect:&str,keys:&dyn KeyStore,key:KeyId)->Result<()> {
        if callback.state.len()!=64||callback.code.is_some()==callback.error.is_some()||callback.code.as_ref().is_some_and(|v|v.is_empty()||v.len()>8192||v.chars().any(char::is_control))||callback.error.as_ref().is_some_and(|v|v.is_empty()||v.len()>128||v.chars().any(char::is_control))||callback.error_description.as_ref().is_some_and(|v|v.len()>4096||v.chars().any(char::is_control)){return Err(invalid());}
        let mut tx=self.pool.begin().await.map_err(storage_error)?;crate::server_keys::cipher_writer(&mut tx,key).await?;
        let id:String=sqlx::query_scalar("SELECT f.id FROM mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id WHERE f.state_sha256=$1 AND p.owner_kind IN('personal','team') AND p.enabled=1 AND p.revision=f.profile_revision AND p.epoch=f.profile_epoch AND f.expires_at>$2 AND f.state IN('awaiting_callback','callback_received','denied')").bind(sha(&callback.state)).bind(now()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let f=lock_flow(&mut tx,&id).await?;let p=lock_profile(&mut tx,&f.view.profile_id).await?;
        if f.view.profile_revision!=p.view.revision||f.profile_epoch!=p.epoch||f.binding_sha256!=p.view.binding_sha256||f.scope!=p.scope(&f.view.id,p.view.revision,"oauth_flow")||p.view.configuration.redirect_uri!=redirect||!p.view.configuration.enabled||callback.issuer.as_ref().is_some_and(|s|s!=&p.authorization.issuer)||p.authorization.original["authorization_response_iss_parameter_supported"]==true&&callback.issuer.as_deref()!=Some(&p.authorization.issuer){return Err(Error::Forbidden);}
        let original:McpOAuthFlowSecret=serde_json::from_slice(&open_mcp_blob(keys,&f.scope,&f.sealed).map_err(|_|Error::NativeKeyUnavailable)?).map_err(storage_error)?;
        if original.state!=callback.state||original.actor!=f.actor["actor"]||original.expires_at!=f.view.expires_at{return Err(Error::Forbidden);}
        let plain=Zeroizing::new(serde_json::to_vec(callback).map_err(storage_error)?);let mut scope=f.scope.clone();scope.purpose="oauth_callback".into();
        if let Some(old)=f.callback {if open_mcp_blob(keys,&scope,&old).map_err(|_|Error::NativeKeyUnavailable)?.as_slice()!=plain.as_slice(){return Err(Error::Conflict);}tx.commit().await.map_err(storage_error)?;return Ok(());}
        if f.view.state!="awaiting_callback"||f.view.expires_at<=now(){return Err(Error::Forbidden);}
        let blob=seal_mcp_blob(keys,key,&scope,&plain).map_err(|_|Error::NativeKeyUnavailable)?;let digest=canonical_hash(&json!([id,sha(&callback.state),callback.code.as_ref().map(|v|sha(v)),callback.error]));
        if sqlx::query("UPDATE mcp_oauth_flows SET state=$1,sealed_callback=$2,callback_sha256=$3 WHERE id=$4 AND state='awaiting_callback' AND sealed_callback IS NULL").bind(if callback.error.is_some(){"denied"}else{"callback_received"}).bind(json_string(&blob)?).bind(digest).bind(id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}tx.commit().await.map_err(storage_error)
    }
    pub async fn native_mcp_oauth_callback_flow(&self, state: &str, key: KeyId) -> Result<String> {
        if !self.is_local_sqlite()
            || state.len() != 64
            || !state.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Forbidden);
        }
        let row=sqlx::query("SELECT f.id FROM mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id WHERE f.state_sha256=$1 AND p.owner_kind='native' AND p.owner_id=$2 AND p.enabled=1 AND p.revision=f.profile_revision AND p.epoch=f.profile_epoch AND f.expires_at>$3 AND f.state IN('awaiting_callback','callback_received')").bind(sha(state)).bind(key.instance.to_string()).bind(now()).fetch_optional(&self.pool).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        row.try_get("id").map_err(storage_error)
    }
    pub async fn finish_captured_mcp_oauth_exchange(
        &self,
        owner: &McpOAuthOwner<'_>,
        profile: &str,
        source: &str,
        kind: &str,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let p = self.mcp_oauth_profile(owner, profile, keys).await?;
        let r = sqlx::query(
            "SELECT * FROM mcp_oauth_exchanges WHERE profile_id=$1 AND kind=$2 AND source_id=$3",
        )
        .bind(profile)
        .bind(kind)
        .bind(source)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        if let Some(receipt) = r
            .try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
        {
            return parse(&receipt);
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? != "received" {
            return Ok(
                json!({"exchange_id":r.try_get::<String,_>("id").map_err(storage_error)?,"state":"unknown","effects_unknown":true}),
            );
        }
        let permit = McpOAuthExchangePermit {
            id: r.try_get("id").map_err(storage_error)?,
            profile: p,
            source_id: source.into(),
            kind: kind.into(),
            request_sha256: r.try_get("request_sha256").map_err(storage_error)?,
            authority_sha256: r.try_get("authority_sha256").map_err(storage_error)?,
            scope: parse(
                &r.try_get::<String, _>("secret_scope")
                    .map_err(storage_error)?,
            )?,
        };
        self.commit_mcp_oauth_exchange(owner, &permit, keys).await
    }
    /// Row fences precede reads and are reacquired after each external await.
    pub(crate) async fn mcp_oauth_authority(
        &self,
        tx: &mut Transaction<'_, Any>,
        owner: &McpOAuthOwner<'_>,
        project: &str,
        server: &str,
        revision: Option<i64>,
        resource: Option<&str>,
        keys: &dyn KeyStore,
    ) -> Result<String> {
        match owner {
            McpOAuthOwner::Native { key } => {
                if !self.is_local_sqlite() {
                    return Err(Error::Forbidden);
                }
                let state = crate::native_secrets::lock_state(tx).await?;
                if state.active.as_ref().is_none_or(|p| p.key != *key)
                    || state.pending_operation.is_some()
                {
                    return Err(Error::NativeKeyUnavailable);
                }
                crate::native_key_retirement::allow_cipher(
                    tx,
                    &key.cipher_id().map_err(|_| Error::Forbidden)?,
                )
                .await?;
                if sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
                    .bind(project)
                    .execute(&mut **tx)
                    .await
                    .map_err(storage_error)?
                    .rows_affected()
                    != 1
                {
                    return Err(Error::NotFound);
                }
                if server.is_empty() {
                    return Ok(canonical_hash(&json!([
                        owner.kind(),
                        owner.id(),
                        project,
                        state.revision
                    ])));
                }
                if sqlx::query(
                    "UPDATE mcp_servers SET revision=revision WHERE id=$1 AND project_id=$2",
                )
                .bind(server)
                .bind(project)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?
                .rows_affected()
                    != 1
                {
                    return Err(Error::NotFound);
                }
                let record=crate::mcp_servers::checked_server(&sqlx::query("SELECT id,project_id,revision,payload FROM mcp_servers WHERE id=$1 AND project_id=$2").bind(server).bind(project).fetch_one(&mut **tx).await.map_err(storage_error)?)?;
                if revision.is_some_and(|v| v != record.revision)
                    || resource.is_some_and(|v| v != record.configuration.endpoint)
                {
                    return Err(Error::Conflict);
                }
                Ok(canonical_hash(&json!([
                    owner.kind(),
                    owner.id(),
                    project,
                    state.revision,
                    record
                ])))
            }
            McpOAuthOwner::Personal { actor, key } => {
                crate::android_devices::device_writer(tx).await?;
                let project_id = Uuid::parse_str(project).map_err(|_| invalid())?;
                self.android_personal_permit(tx, actor, project_id, ContentAction::Write)
                    .await?;
                crate::server_keys::server_admission(tx, *key).await?;
                let r=sqlx::query("SELECT revision,binding_sha,sealed,deleted,metadata_revision,metadata_sha FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0").bind(owner.id()).bind(project).bind(server).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
                if revision.is_some_and(|v| r.try_get::<i64, _>("revision").ok() != Some(v)) {
                    return Err(Error::Conflict);
                }
                let value = crate::android_private_mcp::oauth_configuration(
                    keys,
                    actor.identity_id(),
                    project_id,
                    Uuid::parse_str(server).map_err(|_| invalid())?,
                    &r,
                )?;
                if value["transport"] != "streamable_http"
                    || resource.is_some_and(|v| value["url"].as_str() != Some(v))
                {
                    return Err(Error::Forbidden);
                }
                Ok(canonical_hash(&json!([
                    owner.actor(),
                    project,
                    server,
                    r.try_get::<i64, _>("revision").map_err(storage_error)?,
                    r.try_get::<String, _>("binding_sha")
                        .map_err(storage_error)?
                ])))
            }
            McpOAuthOwner::Team { actor, team, key } => {
                let org = self
                    .identity()
                    .organization_in_transaction(tx, actor, *team)
                    .await
                    .map_err(crate::team_service::identity_error)?;
                self.identity()
                    .require_session_action_in_transaction(tx, actor, ContentAction::Share)
                    .await
                    .map_err(crate::team_service::identity_error)?;
                let role:String=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1").bind(team.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
                if !matches!(role.as_str(), "owner" | "admin") {
                    return Err(Error::Forbidden);
                }
                crate::server_keys::server_admission(tx, *key).await?;
                if sqlx::query(
                    "UPDATE team_mcp_servers SET revision=revision WHERE id=$1 AND team_id=$2",
                )
                .bind(server)
                .bind(team.to_string())
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?
                .rows_affected()
                    != 1
                {
                    return Err(Error::NotFound);
                }
                let r = sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2")
                    .bind(server)
                    .bind(team.to_string())
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(storage_error)?
                    .ok_or(Error::NotFound)?;
                let actual: i64 = r.try_get("revision").map_err(storage_error)?;
                if revision.is_some_and(|v| v != actual) {
                    return Err(Error::Conflict);
                }
                let sealed: cowork_secrets::SealedCredential = parse(
                    &r.try_get::<String, _>("sealed_config")
                        .map_err(storage_error)?,
                )?;
                let scope = cowork_secrets::CredentialScope {
                    project_id: format!("team:{team}"),
                    server_id: server.into(),
                    reference: "team-mcp/v1".into(),
                    revision: actual,
                    endpoint: "team-configuration://team-mcp/v1".into(),
                };
                let plain = cowork_secrets::open_bearer(keys, &scope, &sealed)
                    .map_err(|_| Error::Forbidden)?;
                let config: Value = serde_json::from_str(&plain).map_err(storage_error)?;
                if resource.is_some_and(|v| config["endpoint"].as_str() != Some(v)) {
                    return Err(Error::Forbidden);
                }
                // Team integrations are shared deliberately. No private identity
                // or native-project secret is adopted into this authority.
                if project != format!("team:{team}") {
                    return Err(Error::Forbidden);
                }
                Ok(canonical_hash(&json!([
                    owner.actor(),
                    org,
                    role,
                    server,
                    actual,
                    config["endpoint"]
                ])))
            }
            McpOAuthOwner::TeamExecution { authority, key } => {
                crate::server_keys::server_admission(tx, *key).await?;
                authority
                    .mcp_oauth_execution_fence(tx, project, server, revision, resource, keys)
                    .await
            }
            McpOAuthOwner::PersonalExecution {
                owner: identity,
                run,
                lease,
                key,
            } => {
                let current = crate::android_devices::device_writer(tx).await?;
                let r = crate::android_private_server::leased(tx, *run, *lease, current).await?;
                crate::android_private_server::authority(tx, &r, current).await?;
                if crate::android_private_server::id(&r, "owner_id")? != *identity
                    || crate::android_private_server::id(&r, "project_id")?.to_string() != project
                {
                    return Err(Error::Forbidden);
                }
                crate::server_keys::server_admission(tx, *key).await?;
                let row=sqlx::query("SELECT * FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0").bind(identity.to_string()).bind(project).bind(server).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
                let value = crate::android_private_mcp::oauth_configuration(
                    keys,
                    *identity,
                    Uuid::parse_str(project).map_err(|_| invalid())?,
                    Uuid::parse_str(server).map_err(|_| invalid())?,
                    &row,
                )?;
                if value["transport"] != "streamable_http"
                    || resource.is_some_and(|v| value["url"].as_str() != Some(v))
                    || revision.is_some_and(|v| row.try_get::<i64, _>("revision").ok() != Some(v))
                {
                    return Err(Error::Forbidden);
                }
                let stored: Value =
                    parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
                crate::android_private_mcp::validate_bindings(
                    tx,
                    *identity,
                    Uuid::parse_str(project).map_err(|_| invalid())?,
                    &stored["spec"]["input"],
                )
                .await?;
                if !stored["spec"]["input"]["frozen_mcp_bindings"]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|b| b["entity_id"] == server))
                {
                    return Err(Error::Forbidden);
                }
                Ok(canonical_hash(&json!([
                    run,
                    lease,
                    identity,
                    project,
                    server,
                    row.try_get::<i64, _>("revision").map_err(storage_error)?,
                    row.try_get::<String, _>("binding_sha")
                        .map_err(storage_error)?,
                    stored["spec"]["input"]["provider_binding"],
                    stored["spec"]["input"]["frozen_mcp_bindings"]
                ])))
            }
        }
    }
    async fn oauth_current(
        &self,
        tx: &mut Transaction<'_, Any>,
        owner: &McpOAuthOwner<'_>,
        id: &str,
        keys: &dyn KeyStore,
    ) -> Result<(McpOAuthProfileRecord, String)> {
        let p = lock_profile(tx, id).await?;
        if p.view.owner_kind != owner.kind()
            || p.view.owner_id != owner.id()
            || !p.view.configuration.enabled
        {
            return Err(Error::Forbidden);
        }
        let authority = self
            .mcp_oauth_authority(
                tx,
                owner,
                &p.view.project_id,
                &p.view.server_id,
                None,
                Some(&p.view.configuration.resource),
                keys,
            )
            .await?;
        self.mcp_oauth_published(tx, &p, keys).await?;
        Ok((p, authority))
    }
    async fn mcp_oauth_published(
        &self,
        tx: &mut Transaction<'_, Any>,
        p: &McpOAuthProfileRecord,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let original = McpOAuthBinding {
            id: p.view.id.clone(),
            revision: p.view.revision,
        };
        match p.view.owner_kind.as_str() {
            "native" => {
                let row = sqlx::query("SELECT * FROM mcp_servers WHERE id=$1 AND project_id=$2")
                    .bind(&p.view.server_id)
                    .bind(&p.view.project_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(storage_error)?;
                let record = crate::mcp_servers::checked_server(&row)?;
                if record.configuration.credential_reference.as_deref()
                    != Some(p.reference().as_str())
                    || record.configuration.credential_revision != p.view.revision
                {
                    return Err(Error::Forbidden);
                }
            }
            "personal" => {
                let row=sqlx::query("SELECT * FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0").bind(&p.view.owner_id).bind(&p.view.project_id).bind(&p.view.server_id).fetch_one(&mut **tx).await.map_err(storage_error)?;
                let value = crate::android_private_mcp::oauth_configuration(
                    keys,
                    Uuid::parse_str(&p.view.owner_id).map_err(|_| invalid())?,
                    Uuid::parse_str(&p.view.project_id).map_err(|_| invalid())?,
                    Uuid::parse_str(&p.view.server_id).map_err(|_| invalid())?,
                    &row,
                )?;
                if value["oauth"] != json!(original) {
                    return Err(Error::Forbidden);
                }
            }
            "team" => {
                let row=sqlx::query("SELECT revision,sealed_config FROM team_mcp_servers WHERE id=$1 AND team_id=$2").bind(&p.view.server_id).bind(&p.view.owner_id).fetch_one(&mut **tx).await.map_err(storage_error)?;
                let revision: i64 = row.try_get("revision").map_err(storage_error)?;
                let sealed: cowork_secrets::SealedCredential = parse(
                    &row.try_get::<String, _>("sealed_config")
                        .map_err(storage_error)?,
                )?;
                let scope = cowork_secrets::CredentialScope {
                    project_id: p.view.project_id.clone(),
                    server_id: p.view.server_id.clone(),
                    reference: "team-mcp/v1".into(),
                    revision,
                    endpoint: "team-configuration://team-mcp/v1".into(),
                };
                let value: Value = serde_json::from_str(
                    &cowork_secrets::open_bearer(keys, &scope, &sealed)
                        .map_err(|_| Error::Forbidden)?,
                )
                .map_err(storage_error)?;
                if value["oauth"] != json!(original) || !p.view.configuration.shared_team_consent {
                    return Err(Error::Forbidden);
                }
            }
            _ => return Err(Error::Forbidden),
        }
        Ok(())
    }
    pub async fn configure_mcp_oauth(
        &self,
        owner: &McpOAuthOwner<'_>,
        project: &str,
        input: &ConfigureMcpOAuth,
        metadata: &ProtectedResourceMetadata,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthProfile> {
        if !owner.management() {
            return Err(Error::Forbidden);
        }
        for v in [&input.profile_id, &input.server_id, &input.idempotency_key] {
            valid_text(v, 128)?;
        }
        if input.expected_revision < 0
            || input.expected_revision == i64::MAX
            || input.server_revision <= 0
            || metadata.sha256 != input.metadata_sha256
            || metadata.resource != input.configuration.resource
            || input.configuration.shared_team_consent != (owner.kind() == "team")
        {
            return Err(invalid());
        }
        let selected = metadata
            .authorization_servers
            .iter()
            .find(|m| m.issuer == input.configuration.authorization_server)
            .ok_or_else(invalid)?
            .clone();
        cowork_mcp::oauth::start(
            &selected,
            &input.configuration.client_id,
            &input.configuration.redirect_uri,
            &input.configuration.resource,
            &input.configuration.scopes,
        )
        .map_err(|_| invalid())?;
        if input.configuration.scopes.iter().any(|s| {
            !metadata.scopes_supported.is_empty() && !metadata.scopes_supported.contains(s)
        }) || input
            .client_secret
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.len() > 8192 || v.chars().any(char::is_control))
        {
            return Err(invalid());
        }
        let request = Zeroizing::new(
            serde_json::to_vec(&json!(["mcp-oauth-config-request/v2",owner.kind(), owner.id(), project, input]))
                .map_err(storage_error)?,
        );
        let auth_key = keys
            .key_for_purpose(&owner.key(), cowork_secrets::KeyPurpose::RequestFingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let material = keys
            .load(&auth_key)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let mac = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, material.as_ref()),
            &request,
        );
        let request_sha = mac
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.mcp_oauth_authority(
            &mut tx,
            owner,
            project,
            &input.server_id,
            None,
            Some(&input.configuration.resource),
            keys,
        )
        .await?;
        if let Some(r)=sqlx::query("SELECT request_sha256,request_key_id,receipt FROM mcp_oauth_configuration_operations WHERE owner_kind=$1 AND owner_id=$2 AND project_id=$3 AND request_key=$4").bind(owner.kind()).bind(owner.id()).bind(project).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            let original_key:KeyId=parse(&r.try_get::<String,_>("request_key_id").map_err(storage_error)?)?;
            let key=keys.load(&original_key).map_err(|_|Error::NativeKeyUnavailable)?;let tag=ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256,key.as_ref()),&request);let digest=tag.as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
            if digest!=r.try_get::<String,_>("request_sha256").map_err(storage_error)?{return Err(Error::Conflict);}return parse(&r.try_get::<String,_>("receipt").map_err(storage_error)?);
        }
        self.mcp_oauth_authority(
            &mut tx,
            owner,
            project,
            &input.server_id,
            Some(input.server_revision),
            Some(&input.configuration.resource),
            keys,
        )
        .await?;
        let existing = sqlx::query("SELECT * FROM mcp_oauth_profiles WHERE id=$1")
            .bind(&input.profile_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .as_ref()
            .map(profile_row)
            .transpose()?;
        if existing.as_ref().map_or(0, |p| p.view.revision) != input.expected_revision
            || existing.as_ref().is_some_and(|p| {
                p.view.owner_kind != owner.kind()
                    || p.view.owner_id != owner.id()
                    || p.view.project_id != project
                    || p.view.server_id != input.server_id
            })
        {
            return Err(Error::Conflict);
        }
        if let Some(pending) = existing
            .as_ref()
            .and_then(|p| p.pending_exchange_id.as_ref())
        {
            let state: String = sqlx::query_scalar(
                "SELECT state FROM mcp_oauth_exchanges WHERE id=$1 AND profile_id=$2",
            )
            .bind(pending)
            .bind(&input.profile_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if !input.acknowledge_unknown || state != "unknown" {
                return Err(Error::UncertainEffect);
            }
        } else if input.acknowledge_unknown {
            return Err(Error::Conflict);
        }
        let revision = input.expected_revision + 1;
        let epoch = Uuid::new_v4();
        let binding = canonical_hash(&json!([
            1,
            owner.kind(),
            owner.id(),
            project,
            input.server_id,
            revision,
            epoch,
            input.configuration,
            metadata.sha256
        ]));
        let view = McpOAuthProfile {
            id: input.profile_id.clone(),
            owner_kind: owner.kind().into(),
            owner_id: owner.id(),
            project_id: project.into(),
            server_id: input.server_id.clone(),
            revision,
            configuration: input.configuration.clone(),
            metadata_sha256: metadata.sha256.clone(),
            binding_sha256: binding.clone(),
            state: if input.configuration.enabled {
                "reauthorization_required"
            } else {
                "disabled"
            }
            .into(),
            expires_at: None,
            reauthorization_required: true,
        };
        let scope = McpSecretScope {
            owner_kind: owner.kind().into(),
            owner_id: owner.id(),
            project_id: project.into(),
            server_id: input.server_id.clone(),
            record_id: input.profile_id.clone(),
            revision,
            epoch,
            configuration_sha256: binding,
            purpose: "oauth_profile".into(),
        };
        let secret = input
            .client_secret
            .as_ref()
            .map(|s| {
                seal_mcp_blob(keys, owner.key(), &scope, s.as_bytes())
                    .map_err(|_| Error::NativeKeyUnavailable)
            })
            .transpose()?;
        let original = existing.as_ref().map_or(now(), |_| now());
        sqlx::query("INSERT INTO mcp_oauth_profiles(id,owner_kind,owner_id,project_id,server_id,revision,epoch,enabled,binding_sha256,payload,secret_scope,sealed_client,active_token_id,pending_exchange_id,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,NULL,NULL,$13,$13) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,epoch=excluded.epoch,enabled=excluded.enabled,binding_sha256=excluded.binding_sha256,payload=excluded.payload,secret_scope=excluded.secret_scope,sealed_client=excluded.sealed_client,active_token_id=NULL,pending_exchange_id=NULL,updated_at=excluded.updated_at")
            .bind(&view.id).bind(owner.kind()).bind(owner.id()).bind(project).bind(&view.server_id).bind(revision).bind(epoch.to_string()).bind(i64::from(view.configuration.enabled)).bind(&view.binding_sha256).bind(json_string(&view)?).bind(json_string(&json!({"scope":scope,"authorization":selected,"resource":metadata}))?).bind(secret.as_ref().map(json_string).transpose()?).bind(original).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query(
            "UPDATE mcp_oauth_tokens SET state='revoked' WHERE profile_id=$1 AND state='current'",
        )
        .bind(&view.id)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        if owner.kind() == "native" {
            let mut server = crate::mcp_servers::checked_server(
                &sqlx::query("SELECT * FROM mcp_servers WHERE id=$1 AND project_id=$2")
                    .bind(&view.server_id)
                    .bind(project)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?,
            )?;
            server.revision = server.revision.checked_add(1).ok_or(Error::Conflict)?;
            server.configuration.credential_reference = Some(format!("oauth:{}", view.id));
            server.configuration.credential_revision = revision;
            server.configuration_sha256 = canonical_hash(&server.configuration);
            server.updated_at = now();
            sqlx::query(
                "UPDATE mcp_servers SET revision=$1,payload=$2 WHERE id=$3 AND project_id=$4",
            )
            .bind(server.revision)
            .bind(json_string(&server)?)
            .bind(&view.server_id)
            .bind(project)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        if let McpOAuthOwner::Personal { actor, key } = owner {
            crate::android_private_mcp::attach_oauth(
                &mut tx,
                keys,
                *key,
                actor.identity_id(),
                Uuid::parse_str(project).map_err(|_| invalid())?,
                Uuid::parse_str(&view.server_id).map_err(|_| invalid())?,
                &McpOAuthBinding {
                    id: view.id.clone(),
                    revision: view.revision,
                },
            )
            .await?;
        } else if let McpOAuthOwner::Team { team, key, .. } = owner {
            let row =
                sqlx::query("SELECT revision FROM team_mcp_servers WHERE id=$1 AND team_id=$2")
                    .bind(&view.server_id)
                    .bind(team.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            let revision: i64 = row.try_get("revision").map_err(storage_error)?;
            let next = revision.checked_add(1).ok_or(Error::Conflict)?;
            let scope = cowork_secrets::CredentialScope {
                project_id: project.into(),
                server_id: view.server_id.clone(),
                reference: "team-mcp/v1".into(),
                revision: next,
                endpoint: "team-configuration://team-mcp/v1".into(),
            };
            let config = json!({"endpoint":view.configuration.resource,"bearer":null,"oauth":{"id":view.id,"revision":view.revision}});
            let sealed = cowork_secrets::seal_bearer(keys, *key, &scope, &json_string(&config)?)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            crate::server_keys::cipher_writer(&mut tx, sealed.key_id()).await?;
            if sqlx::query("UPDATE team_mcp_servers SET revision=$1,sealed_config=$2,catalog=NULL,catalog_sha256=NULL,sealed_session=NULL WHERE id=$3 AND team_id=$4 AND revision=$5").bind(next).bind(json_string(&sealed)?).bind(&view.server_id).bind(team.to_string()).bind(revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        }
        sqlx::query("INSERT INTO mcp_oauth_configuration_operations(owner_kind,owner_id,project_id,request_key,request_sha256,request_key_id,profile_id,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(owner.kind()).bind(owner.id()).bind(project).bind(&input.idempotency_key).bind(request_sha).bind(json_string(&auth_key)?).bind(&view.id).bind(json_string(&view)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(view)
    }
    pub async fn mcp_oauth_profiles(
        &self,
        owner: &McpOAuthOwner<'_>,
        project: &str,
        keys: &dyn KeyStore,
    ) -> Result<Vec<McpOAuthProfile>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let rows=sqlx::query("SELECT * FROM mcp_oauth_profiles WHERE owner_kind=$1 AND owner_id=$2 AND project_id=$3 ORDER BY id LIMIT 129").bind(owner.kind()).bind(owner.id()).bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        let mut result = Vec::new();
        for row in rows {
            let p = profile_row(&row)?;
            self.mcp_oauth_authority(
                &mut tx,
                owner,
                project,
                &p.view.server_id,
                None,
                Some(&p.view.configuration.resource),
                keys,
            )
            .await?;
            result.push(p.view);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn mcp_oauth_profile(
        &self,
        owner: &McpOAuthOwner<'_>,
        id: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthProfileRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, _) = self.oauth_current(&mut tx, owner, id, keys).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(p)
    }
    pub async fn start_mcp_oauth(
        &self,
        owner: &McpOAuthOwner<'_>,
        id: &str,
        input: &McpOAuthRequest,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthFlow> {
        if !owner.management() {
            return Err(Error::Forbidden);
        }
        valid_text(&input.idempotency_key, 128)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, _) = self.oauth_current(&mut tx, owner, id, keys).await?;
        if p.view.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        if p.pending_exchange_id.is_some() {
            return Err(Error::UncertainEffect);
        }
        let actor=original_actor(&mut tx,owner).await?;
        let fid = Uuid::new_v4().to_string();
        let start = cowork_mcp::oauth::start(
            &p.authorization,
            &p.view.configuration.client_id,
            &p.view.configuration.redirect_uri,
            &p.view.configuration.resource,
            &p.view.configuration.scopes,
        )
        .map_err(|_| invalid())?;
        let expiry = now() + 600_000;
        let scope = p.scope(&fid, p.view.revision, "oauth_flow");
        let secret = McpOAuthFlowSecret {
            state: start.state.to_string(),
            verifier: start.verifier.to_string(),
            authorization_url: start.authorization_url.clone(),
            expires_at: expiry,
            actor: actor.clone(),
        };
        let sealed = seal_mcp_blob(
            keys,
            owner.key(),
            &scope,
            &Zeroizing::new(serde_json::to_vec(&secret).map_err(storage_error)?),
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        // Start is local and repeatable, but its request key names exactly one
        // browser handoff. A replay returns the original URL and state.
        let flow_key = canonical_hash(&json!([
            actor,
            id,
            p.view.revision,
            input.idempotency_key
        ]));
        if let Some(r) =
            sqlx::query("SELECT id FROM mcp_oauth_flows WHERE profile_id=$1 AND original_actor=$2")
                .bind(id)
                .bind(json_string(
                    &json!({"actor":actor,"request":flow_key}),
                )?)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            let original = r.try_get::<String, _>("id").map_err(storage_error)?;
            let f = flow_row(
                &sqlx::query("SELECT * FROM mcp_oauth_flows WHERE id=$1")
                    .bind(original)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?,
            )?;
            let v: McpOAuthFlowSecret = serde_json::from_slice(
                &open_mcp_blob(keys, &f.scope, &f.sealed)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(storage_error)?;
            let mut view = f.view;
            view.authorization_url = Some(v.authorization_url.clone());
            return Ok(view);
        }
        if p.active_token_id.is_some() { return Err(Error::Conflict); }
        sqlx::query("INSERT INTO mcp_oauth_flows(id,profile_id,profile_revision,profile_epoch,binding_sha256,original_actor,redirect_uri,metadata_sha256,state_sha256,state,secret_scope,sealed_flow,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'awaiting_callback',$10,$11,$12,$13)").bind(&fid).bind(id).bind(p.view.revision).bind(p.epoch.to_string()).bind(&p.view.binding_sha256).bind(json_string(&json!({"actor":actor,"request":flow_key}))?).bind(&p.view.configuration.redirect_uri).bind(&p.view.metadata_sha256).bind(sha(&start.state)).bind(json_string(&scope)?).bind(json_string(&sealed)?).bind(expiry).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(McpOAuthFlow {
            id: fid,
            profile_id: id.into(),
            profile_revision: p.view.revision,
            state: "awaiting_callback".into(),
            authorization_url: Some(start.authorization_url),
            expires_at: expiry,
            callback_received: false,
        })
    }
    pub async fn capture_mcp_oauth_callback(
        &self,
        owner: &McpOAuthOwner<'_>,
        flow: &str,
        callback: &McpOAuthCallback,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthFlow> {
        if !owner.management() {
            return Err(Error::Forbidden);
        }
        if callback.state.len() != 64
            || callback
                .code
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 8192 || v.chars().any(char::is_control))
            || callback.code.is_some() == callback.error.is_some()
        {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let f = lock_flow(&mut tx, flow).await?;
        let (p, _) = self
            .oauth_current(&mut tx, owner, &f.view.profile_id, keys)
            .await?;
        check_flow(&mut tx, &p, &f, owner).await?;
        if callback
            .issuer
            .as_ref()
            .is_some_and(|v| v != &p.authorization.issuer)
            || p.authorization.original["authorization_response_iss_parameter_supported"] == true
                && callback.issuer.as_deref() != Some(&p.authorization.issuer)
            || callback
                .error_description
                .as_ref()
                .is_some_and(|v| v.len() > 4096 || v.chars().any(char::is_control))
        {
            return Err(Error::Forbidden);
        }
        let original: McpOAuthFlowSecret = serde_json::from_slice(
            &open_mcp_blob(keys, &f.scope, &f.sealed).map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(storage_error)?;
        if original.state != callback.state
            || original.expires_at != f.view.expires_at
            || original.actor != f.actor["actor"]
        {
            return Err(Error::Forbidden);
        }
        let plain = Zeroizing::new(serde_json::to_vec(callback).map_err(storage_error)?);
        let digest = canonical_hash(&json!([
            flow,
            sha(&callback.state),
            callback.code.as_ref().map(|v| sha(v)),
            callback.error
        ]));
        if let Some(old) = f.callback {
            let mut scope = f.scope.clone();
            scope.purpose = "oauth_callback".into();
            let bytes =
                open_mcp_blob(keys, &scope, &old).map_err(|_| Error::NativeKeyUnavailable)?;
            if bytes.as_slice() != plain.as_slice() {
                return Err(Error::Conflict);
            }
            return Ok(f.view);
        }
        if f.view.state != "awaiting_callback" || f.view.expires_at <= now() {
            return Err(Error::Forbidden);
        }
        let mut scope = f.scope;
        scope.purpose = "oauth_callback".into();
        let sealed = seal_mcp_blob(keys, owner.key(), &scope, &plain)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let state = if callback.error.is_some() {
            "denied"
        } else {
            "callback_received"
        };
        sqlx::query("UPDATE mcp_oauth_flows SET state=$1,sealed_callback=$2,callback_sha256=$3 WHERE id=$4 AND state='awaiting_callback'").bind(state).bind(json_string(&sealed)?).bind(digest).bind(flow).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        let mut view = f.view;
        view.state = state.into();
        view.callback_received = true;
        Ok(view)
    }
    pub async fn mcp_oauth_flow(
        &self,
        owner: &McpOAuthOwner<'_>,
        flow: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthFlowRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut f = lock_flow(&mut tx, flow).await?;
        let (p, _) = self
            .oauth_current(&mut tx, owner, &f.view.profile_id, keys)
            .await?;
        check_flow(&mut tx, &p, &f, owner).await?;
        if f.view.expires_at<=now()&&matches!(f.view.state.as_str(),"awaiting_callback"|"callback_received"){
            if sqlx::query("UPDATE mcp_oauth_flows SET state='expired' WHERE id=$1 AND state=$2 AND expires_at<=$3").bind(flow).bind(&f.view.state).bind(now()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}f.view.state="expired".into();
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(f)
    }
    pub async fn mcp_oauth_token(
        &self,
        owner: &McpOAuthOwner<'_>,
        id: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthTokenRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, _) = self.oauth_current(&mut tx, owner, id, keys).await?;
        if p.pending_exchange_id.is_some() {
            return Err(Error::UncertainEffect);
        }
        let token = token_row(
            &sqlx::query("SELECT * FROM mcp_oauth_tokens WHERE id=$1")
                .bind(
                    p.active_token_id
                        .as_ref()
                        .ok_or(Error::McpSessionUnavailable)?,
                )
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        )?;
        check_token(&p, &token)?;
        if token.state != "current" {
            return Err(Error::McpSessionUnavailable);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(token)
    }
    pub async fn begin_mcp_oauth_exchange(
        &self,
        owner: &McpOAuthOwner<'_>,
        id: &str,
        source: &str,
        kind: &str,
        request_sha: &str,
        keys: &dyn KeyStore,
    ) -> Result<McpOAuthExchangeAdmission> {
        if !matches!(kind, "authorization_code" | "refresh_token") || request_sha.len() != 64 {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, authority) = self.oauth_current(&mut tx, owner, id, keys).await?;
        if let Some(r)=sqlx::query("SELECT request_sha256,receipt,state FROM mcp_oauth_exchanges WHERE profile_id=$1 AND kind=$2 AND source_id=$3").bind(id).bind(kind).bind(source).fetch_optional(&mut *tx).await.map_err(storage_error)? {if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request_sha{return Err(Error::Conflict);}let receipt=r.try_get::<Option<String>,_>("receipt").map_err(storage_error)?.map(|v|parse(&v)).transpose()?.unwrap_or_else(||json!({"state":r.try_get::<String,_>("state").unwrap_or("unknown".into()),"effects_unknown":true}));return Ok(McpOAuthExchangeAdmission::Original(receipt));}
        if p.pending_exchange_id.is_some() {
            return Err(Error::UncertainEffect);
        }
        if kind == "authorization_code" {
            if !owner.management() || p.active_token_id.is_some() {
                return Err(Error::Forbidden);
            }
            let f = lock_flow(&mut tx, source).await?;
            check_flow(&mut tx, &p, &f, owner).await?;
            if f.view.state != "callback_received" || f.view.expires_at <= now() {
                return Err(Error::Forbidden);
            }
            sqlx::query("UPDATE mcp_oauth_flows SET state='exchange_admitted' WHERE id=$1")
                .bind(source)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
        } else {
            let token = token_row(
                &sqlx::query("SELECT * FROM mcp_oauth_tokens WHERE id=$1")
                    .bind(source)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage_error)?
                    .ok_or(Error::NotFound)?,
            )?;
            check_token(&p, &token)?;
            if token.state != "current" || p.active_token_id.as_deref() != Some(source) {
                return Err(Error::Forbidden);
            }
            sqlx::query("UPDATE mcp_oauth_tokens SET state='refresh_admitted' WHERE id=$1 AND state='current'").bind(source).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let eid = Uuid::new_v4().to_string();
        let scope = p.scope(&eid, 1, "oauth_response");
        sqlx::query("INSERT INTO mcp_oauth_exchanges(id,profile_id,profile_revision,profile_epoch,kind,source_id,request_sha256,authority_sha256,state,secret_scope,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'admitted',$9,$10,$10)").bind(&eid).bind(id).bind(p.view.revision).bind(p.epoch.to_string()).bind(kind).bind(source).bind(request_sha).bind(&authority).bind(json_string(&scope)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE mcp_oauth_profiles SET pending_exchange_id=$1 WHERE id=$2")
            .bind(&eid)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(McpOAuthExchangeAdmission::Dispatch(
            McpOAuthExchangePermit {
                id: eid,
                profile: p,
                source_id: source.into(),
                kind: kind.into(),
                request_sha256: request_sha.into(),
                authority_sha256: authority,
                scope,
            },
        ))
    }
    pub async fn validate_mcp_oauth_exchange(
        &self,
        owner: &McpOAuthOwner<'_>,
        permit: &McpOAuthExchangePermit,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, authority) = self
            .oauth_current(&mut tx, owner, &permit.profile.view.id, keys)
            .await?;
        check_exchange(&mut tx, &p, permit).await?;
        if authority != permit.authority_sha256 {
            return Err(Error::Forbidden);
        }
        let state: String = sqlx::query_scalar("SELECT state FROM mcp_oauth_exchanges WHERE id=$1")
            .bind(&permit.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if state != "admitted" {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn capture_mcp_oauth_exchange(
        &self,
        permit: &McpOAuthExchangePermit,
        response: &Value,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        // Capture is evidence custody, deliberately possible after authority was
        // revoked. Adoption below separately requires fresh current authority.
        let plain = Zeroizing::new(serde_json::to_vec(response).map_err(storage_error)?);
        let sealed = seal_mcp_blob(keys, key, &permit.scope, &plain)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let digest = canonical_hash(response);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        evidence_cipher(&mut tx, &permit.scope.owner_kind, key).await?;
        let r=sqlx::query("SELECT state,request_sha256,secret_scope,sealed_response,response_sha256 FROM mcp_oauth_exchanges WHERE id=$1").bind(&permit.id).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != permit.request_sha256
            || parse::<McpSecretScope>(
                &r.try_get::<String, _>("secret_scope")
                    .map_err(storage_error)?,
            )? != permit.scope
        {
            return Err(Error::Forbidden);
        }
        if let Some(old) = r
            .try_get::<Option<String>, _>("sealed_response")
            .map_err(storage_error)?
        {
            let old: SealedMcpBlob = parse(&old)?;
            if open_mcp_blob(keys, &permit.scope, &old)
                .map_err(|_| Error::NativeKeyUnavailable)?
                .as_slice()
                != plain.as_slice()
            {
                return Err(Error::Conflict);
            }
            return Ok(());
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? != "admitted" {
            return Err(Error::Conflict);
        }
        if sqlx::query("UPDATE mcp_oauth_exchanges SET state='received',sealed_response=$1,response_sha256=$2,updated_at=$3 WHERE id=$4 AND state='admitted' AND sealed_response IS NULL").bind(json_string(&sealed)?).bind(digest).bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn commit_mcp_oauth_exchange(
        &self,
        owner: &McpOAuthOwner<'_>,
        permit: &McpOAuthExchangePermit,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, authority) = self
            .oauth_current(&mut tx, owner, &permit.profile.view.id, keys)
            .await?;
        check_exchange(&mut tx, &p, permit).await?;
        if authority != permit.authority_sha256 {
            return Err(Error::Forbidden);
        }
        let r=sqlx::query("SELECT state,sealed_response,response_sha256,receipt FROM mcp_oauth_exchanges WHERE id=$1").bind(&permit.id).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if let Some(receipt) = r
            .try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
        {
            return parse(&receipt);
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? != "received" {
            return Err(Error::UncertainEffect);
        }
        let sealed: SealedMcpBlob = parse(
            &r.try_get::<String, _>("sealed_response")
                .map_err(storage_error)?,
        )?;
        let raw =
            open_mcp_blob(keys, &permit.scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let response: Value = serde_json::from_slice(&raw).map_err(storage_error)?;
        if canonical_hash(&response)
            != r.try_get::<String, _>("response_sha256")
                .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let parsed = cowork_mcp::oauth::OAuthResponse {
            status: response["status"]
                .as_u64()
                .and_then(|v| u16::try_from(v).ok())
                .ok_or(Error::Forbidden)?,
            body: response["body"].clone(),
        };
        let values = parsed.token_values(&p.view.configuration.scopes);
        let receipt = match values {
            Ok(values) => {
                let token_id = Uuid::new_v4().to_string();
                let (previous, grant, revision) = if permit.kind == "refresh_token" {
                    let old = token_row(
                        &sqlx::query("SELECT * FROM mcp_oauth_tokens WHERE id=$1")
                            .bind(&permit.source_id)
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(storage_error)?,
                    )?;
                    check_token(&p, &old)?;
                    (
                        Some(old.id),
                        old.grant_id,
                        old.revision.checked_add(1).ok_or(Error::Conflict)?,
                    )
                } else {
                    (None, Uuid::new_v4().to_string(), 1)
                };
                let scope = p.scope(&token_id, revision, "oauth_token");
                let secret = McpOAuthTokenSecret {
                    access: values.access.to_string(),
                    refresh: values.refresh.as_ref().map(|v| v.to_string()),
                };
                // No refresh token in the response retires refresh ability; an older
                // token is never silently adopted after an exchange.
                let blob = seal_mcp_blob(
                    keys,
                    owner.key(),
                    &scope,
                    &Zeroizing::new(serde_json::to_vec(&secret).map_err(storage_error)?),
                )
                .map_err(|_| Error::NativeKeyUnavailable)?;
                let expiry = now()
                    .checked_add(values.client_use_ms as i64)
                    .ok_or(Error::Forbidden)?;
                sqlx::query("INSERT INTO mcp_oauth_tokens(id,profile_id,profile_revision,profile_epoch,binding_sha256,grant_id,revision,state,expires_at,scopes,secret_scope,sealed_token,original_exchange_id,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'current',$8,$9,$10,$11,$12,$13)").bind(&token_id).bind(&p.view.id).bind(p.view.revision).bind(p.epoch.to_string()).bind(&p.view.binding_sha256).bind(grant).bind(revision).bind(expiry).bind(json_string(&values.scopes)?).bind(json_string(&scope)?).bind(json_string(&blob)?).bind(&permit.id).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
                if let Some(previous) = previous {
                    sqlx::query("UPDATE mcp_oauth_tokens SET state='retired' WHERE id=$1")
                        .bind(previous)
                        .execute(&mut *tx)
                        .await
                        .map_err(storage_error)?;
                }
                let mut view = p.view.clone();
                view.state = "connected".into();
                view.expires_at = Some(expiry);
                view.reauthorization_required = false;
                sqlx::query("UPDATE mcp_oauth_profiles SET active_token_id=$1,pending_exchange_id=NULL,payload=$2 WHERE id=$3").bind(token_id).bind(json_string(&view)?).bind(&p.view.id).execute(&mut *tx).await.map_err(storage_error)?;
                json!({"exchange_id":permit.id,"profile_id":p.view.id,"state":"connected","token_revision":revision,"expires_at":expiry,"response_sha256":canonical_hash(&response)})
            }
            Err(_) => {
                let mut view = p.view.clone();
                view.state = "reauthorization_required".into();
                view.reauthorization_required = true;
                view.expires_at = None;
                sqlx::query("UPDATE mcp_oauth_profiles SET active_token_id=NULL,pending_exchange_id=NULL,payload=$1 WHERE id=$2").bind(json_string(&view)?).bind(&p.view.id).execute(&mut *tx).await.map_err(storage_error)?;
                json!({"exchange_id":permit.id,"profile_id":p.view.id,"state":"reauthorization_required","response_sha256":canonical_hash(&response)})
            }
        };
        sqlx::query("UPDATE mcp_oauth_exchanges SET state=$1,receipt=$2,updated_at=$3 WHERE id=$4 AND state='received'").bind(if receipt["state"]=="connected"{"committed"}else{"rejected"}).bind(json_string(&receipt)?).bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?;
        if permit.kind == "authorization_code" {
            sqlx::query("UPDATE mcp_oauth_flows SET state=$1,receipt=$2 WHERE id=$3 AND state='exchange_admitted'").bind(if receipt["state"]=="connected"{"connected"}else{"denied"}).bind(json_string(&receipt)?).bind(&permit.source_id).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn unknown_mcp_oauth_exchange(
        &self,
        permit: &McpOAuthExchangePermit,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let receipt = json!({"exchange_id":permit.id,"profile_id":permit.profile.view.id,"state":"unknown","reauthorization_required":true,"effects_unknown":true});
        if sqlx::query("UPDATE mcp_oauth_exchanges SET state='unknown',receipt=$1,updated_at=$2 WHERE id=$3 AND state='admitted' AND sealed_response IS NULL").bind(json_string(&receipt)?).bind(now()).bind(&permit.id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()==1{
            let mut view=permit.profile.view.clone();view.state="unknown".into();view.reauthorization_required=true;view.expires_at=None;sqlx::query("UPDATE mcp_oauth_profiles SET active_token_id=NULL,payload=$1 WHERE id=$2 AND pending_exchange_id=$3 AND revision=$4").bind(json_string(&view)?).bind(&view.id).bind(&permit.id).bind(view.revision).execute(&mut *tx).await.map_err(storage_error)?;
            sqlx::query("UPDATE mcp_oauth_tokens SET state='unknown' WHERE id=$1 AND state='refresh_admitted'").bind(&permit.source_id).execute(&mut *tx).await.map_err(storage_error)?;
            if permit.kind=="authorization_code"{sqlx::query("UPDATE mcp_oauth_flows SET state='unknown',receipt=$1 WHERE id=$2 AND profile_id=$3 AND state='exchange_admitted'").bind(json_string(&receipt)?).bind(&permit.source_id).bind(&permit.profile.view.id).execute(&mut *tx).await.map_err(storage_error)?;}
        }else{return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
fn flow_row(r: &AnyRow) -> Result<McpOAuthFlowRecord> {
    Ok(McpOAuthFlowRecord {
        view: McpOAuthFlow {
            id: r.try_get("id").map_err(storage_error)?,
            profile_id: r.try_get("profile_id").map_err(storage_error)?,
            profile_revision: r.try_get("profile_revision").map_err(storage_error)?,
            state: r.try_get("state").map_err(storage_error)?,
            authorization_url: None,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
            callback_received: r
                .try_get::<Option<String>, _>("sealed_callback")
                .map_err(storage_error)?
                .is_some(),
        },
        profile_epoch: Uuid::parse_str(
            &r.try_get::<String, _>("profile_epoch")
                .map_err(storage_error)?,
        )
        .map_err(|_| Error::Forbidden)?,
        binding_sha256: r.try_get("binding_sha256").map_err(storage_error)?,
        actor: parse(
            &r.try_get::<String, _>("original_actor")
                .map_err(storage_error)?,
        )?,
        scope: parse(
            &r.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )?,
        sealed: parse(
            &r.try_get::<String, _>("sealed_flow")
                .map_err(storage_error)?,
        )?,
        callback: r
            .try_get::<Option<String>, _>("sealed_callback")
            .map_err(storage_error)?
            .map(|v| parse(&v))
            .transpose()?,
    })
}
async fn lock_flow(tx: &mut Transaction<'_, Any>, id: &str) -> Result<McpOAuthFlowRecord> {
    if sqlx::query("UPDATE mcp_oauth_flows SET state=state WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    flow_row(
        &sqlx::query("SELECT * FROM mcp_oauth_flows WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?,
    )
}
async fn check_flow(
    tx:&mut Transaction<'_,Any>,
    p: &McpOAuthProfileRecord,
    f: &McpOAuthFlowRecord,
    owner: &McpOAuthOwner<'_>,
) -> Result<()> {
    if f.view.profile_id != p.view.id
        || f.view.profile_revision != p.view.revision
        || f.profile_epoch != p.epoch
        || f.binding_sha256 != p.view.binding_sha256
        || f.actor["actor"] != original_actor(tx,owner).await?
        || f.scope != p.scope(&f.view.id, p.view.revision, "oauth_flow")
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
fn token_row(r: &AnyRow) -> Result<McpOAuthTokenRecord> {
    Ok(McpOAuthTokenRecord {
        id: r.try_get("id").map_err(storage_error)?,
        profile_id: r.try_get("profile_id").map_err(storage_error)?,
        revision: r.try_get("revision").map_err(storage_error)?,
        grant_id: r.try_get("grant_id").map_err(storage_error)?,
        state: r.try_get("state").map_err(storage_error)?,
        expires_at: r.try_get("expires_at").map_err(storage_error)?,
        scopes: parse(&r.try_get::<String, _>("scopes").map_err(storage_error)?)?,
        scope: parse(
            &r.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )?,
        sealed: parse(
            &r.try_get::<String, _>("sealed_token")
                .map_err(storage_error)?,
        )?,
    })
}
fn check_token(p: &McpOAuthProfileRecord, t: &McpOAuthTokenRecord) -> Result<()> {
    if t.profile_id != p.view.id
        || t.scope != p.scope(&t.id, t.revision, "oauth_token")
        || t.scopes
            .iter()
            .any(|s| !p.view.configuration.scopes.contains(s))
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn check_exchange(
    tx: &mut Transaction<'_, Any>,
    p: &McpOAuthProfileRecord,
    permit: &McpOAuthExchangePermit,
) -> Result<()> {
    if p.view.revision != permit.profile.view.revision
        || p.epoch != permit.profile.epoch
        || p.view.binding_sha256 != permit.profile.view.binding_sha256
        || p.pending_exchange_id.as_deref() != Some(&permit.id)
        || permit.scope != p.scope(&permit.id, 1, "oauth_response")
    {
        return Err(Error::Forbidden);
    }
    let r=sqlx::query("SELECT profile_id,profile_revision,profile_epoch,kind,source_id,request_sha256,authority_sha256,secret_scope FROM mcp_oauth_exchanges WHERE id=$1").bind(&permit.id).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if r.try_get::<String, _>("profile_id")
        .map_err(storage_error)?
        != p.view.id
        || r.try_get::<i64, _>("profile_revision")
            .map_err(storage_error)?
            != p.view.revision
        || r.try_get::<String, _>("profile_epoch")
            .map_err(storage_error)?
            != p.epoch.to_string()
        || r.try_get::<String, _>("kind").map_err(storage_error)? != permit.kind
        || r.try_get::<String, _>("source_id").map_err(storage_error)? != permit.source_id
        || r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != permit.request_sha256
        || r.try_get::<String, _>("authority_sha256")
            .map_err(storage_error)?
            != permit.authority_sha256
        || parse::<McpSecretScope>(
            &r.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )? != permit.scope
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

/// Tool admission shares the profile/token row fences with refresh and revoke.
pub(crate) async fn check_native_server(
    tx: &mut Transaction<'_, Any>,
    server: &cowork_domain::mcp::McpServerRecord,
) -> Result<()> {
    let reference = server
        .configuration
        .credential_reference
        .as_deref()
        .ok_or(Error::Forbidden)?;
    let id = reference.strip_prefix("oauth:").ok_or(Error::Forbidden)?;
    let p = lock_profile(tx, id).await?;
    let state = crate::native_secrets::lock_state(tx).await?;
    if p.view.owner_kind != "native"
        || p.view.owner_id != state.instance.to_string()
        || p.view.project_id != server.project_id
        || p.view.server_id != server.id
        || p.view.revision != server.configuration.credential_revision
        || p.view.configuration.resource != server.configuration.endpoint
        || !p.view.configuration.enabled
        || p.pending_exchange_id.is_some()
    {
        return Err(Error::Forbidden);
    }
    let id = p.active_token_id.ok_or(Error::McpSessionUnavailable)?;
    if sqlx::query("UPDATE mcp_oauth_tokens SET revision=revision WHERE id=$1 AND state='current' AND expires_at>$2").bind(&id).bind(now()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::McpSessionUnavailable);}
    Ok(())
}
pub(crate) async fn evidence_cipher(
    tx: &mut Transaction<'_, Any>,
    kind: &str,
    key: KeyId,
) -> Result<()> {
    if kind == "native" {
        let state = crate::native_secrets::lock_state(tx).await?;
        if state.active.as_ref().is_none_or(|p| p.key != key) || state.pending_operation.is_some() {
            return Err(Error::NativeKeyUnavailable);
        }
        crate::native_key_retirement::allow_cipher(
            tx,
            &key.cipher_id().map_err(|_| Error::Forbidden)?,
        )
        .await
    } else if kind == "offline" {
        crate::offline_mcp_tasks::cipher_writer(tx, key).await
    } else if matches!(kind, "personal" | "team") {
        crate::server_keys::cipher_writer(tx, key).await
    } else {
        Err(Error::Forbidden)
    }
}

impl Store {
    async fn mcp_oauth_once(
        &self,
        owner: &McpOAuthOwner<'_>,
        profile: &str,
        source: &str,
        kind: &str,
        request: cowork_mcp::oauth::TokenRequest,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let permit = match self
            .begin_mcp_oauth_exchange(owner, profile, source, kind, request.sha256(), keys)
            .await?
        {
            McpOAuthExchangeAdmission::Original(_) => {
                return self
                    .finish_captured_mcp_oauth_exchange(owner, profile, source, kind, keys)
                    .await;
            }
            McpOAuthExchangeAdmission::Dispatch(permit) => permit,
        };
        self.validate_mcp_oauth_exchange(owner, &permit, keys)
            .await?;
        let network = if owner.kind() == "native" {
            cowork_mcp::oauth::OAuthNetwork::NativeOwner
        } else {
            cowork_mcp::oauth::OAuthNetwork::PublicServer
        };
        let received = match request.dispatch(network).await {
            Ok(response) => json!({"status":response.status,"body":response.body}),
            Err(_) => return self.unknown_mcp_oauth_exchange(&permit).await,
        };
        self.capture_mcp_oauth_exchange(&permit, &received, keys, owner.key())
            .await?;
        self.commit_mcp_oauth_exchange(owner, &permit, keys).await
    }
    pub async fn complete_mcp_oauth_code(
        &self,
        owner: &McpOAuthOwner<'_>,
        flow: &str,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        if !owner.management() {
            return Err(Error::Forbidden);
        }
        let f = self.mcp_oauth_flow(owner, flow, keys).await?;
        let p = self
            .mcp_oauth_profile(owner, &f.view.profile_id, keys)
            .await?;
        let secret: McpOAuthFlowSecret = serde_json::from_slice(
            &open_mcp_blob(keys, &f.scope, &f.sealed).map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(storage_error)?;
        if secret.actor != f.actor["actor"] || secret.expires_at != f.view.expires_at {
            return Err(Error::Forbidden);
        }
        let mut scope = f.scope.clone();
        scope.purpose = "oauth_callback".into();
        let callback: McpOAuthCallback = serde_json::from_slice(
            &open_mcp_blob(keys, &scope, f.callback.as_ref().ok_or(Error::Conflict)?)
                .map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(storage_error)?;
        if callback.state != secret.state {
            return Err(Error::Forbidden);
        }
        let client = p
            .sealed_client
            .as_ref()
            .map(|s| {
                open_mcp_blob(keys, &p.client_scope, s)
                    .and_then(|v| {
                        String::from_utf8(v.to_vec())
                            .map(Zeroizing::new)
                            .map_err(|_| cowork_secrets::Error::Invalid)
                    })
                    .map_err(|_| Error::NativeKeyUnavailable)
            })
            .transpose()?;
        let request = cowork_mcp::oauth::TokenRequest::authorization_code(
            &p.authorization,
            &p.view.configuration.client_id,
            client,
            &p.view.configuration.redirect_uri,
            &p.view.configuration.resource,
            Zeroizing::new(callback.code.as_ref().ok_or(Error::Forbidden)?.clone()),
            Zeroizing::new(secret.verifier.clone()),
        )
        .map_err(|_| Error::Forbidden)?;
        self.mcp_oauth_once(owner, &p.view.id, flow, "authorization_code", request, keys)
            .await
    }
    pub async fn mcp_oauth_bearer_for(
        &self,
        owner: &McpOAuthOwner<'_>,
        profile: &str,
        keys: &dyn KeyStore,
    ) -> Result<Zeroizing<String>> {
        let p = self.mcp_oauth_profile(owner, profile, keys).await?;
        let token = self.mcp_oauth_token(owner, profile, keys).await?;
        let secret: McpOAuthTokenSecret = serde_json::from_slice(
            &open_mcp_blob(keys, &token.scope, &token.sealed)
                .map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(storage_error)?;
        if token.expires_at <= now() + 30_000 {
            let refresh = secret
                .refresh
                .as_ref()
                .ok_or(Error::McpSessionUnavailable)?;
            let client = p
                .sealed_client
                .as_ref()
                .map(|s| {
                    open_mcp_blob(keys, &p.client_scope, s)
                        .and_then(|v| {
                            String::from_utf8(v.to_vec())
                                .map(Zeroizing::new)
                                .map_err(|_| cowork_secrets::Error::Invalid)
                        })
                        .map_err(|_| Error::NativeKeyUnavailable)
                })
                .transpose()?;
            let request = cowork_mcp::oauth::TokenRequest::refresh(
                &p.authorization,
                &p.view.configuration.client_id,
                client,
                &p.view.configuration.resource,
                Zeroizing::new(refresh.clone()),
                &token.scopes,
            )
            .map_err(|_| Error::Forbidden)?;
            let receipt = self
                .mcp_oauth_once(owner, profile, &token.id, "refresh_token", request, keys)
                .await?;
            if receipt["state"] != "connected" {
                return Err(Error::McpSessionUnavailable);
            }
            let current = self.mcp_oauth_token(owner, profile, keys).await?;
            if current.expires_at <= now() {
                return Err(Error::McpSessionUnavailable);
            }
            let secret: McpOAuthTokenSecret = serde_json::from_slice(
                &open_mcp_blob(keys, &current.scope, &current.sealed)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(storage_error)?;
            return Ok(Zeroizing::new(secret.access.clone()));
        }
        Ok(Zeroizing::new(secret.access.clone()))
    }
}

/// Both inventory and rewrap include original flow/response/task custody. A
/// token/refresh operation in flight must settle before changing cipher custody.
pub(crate) async fn required_keys(tx: &mut Transaction<'_, Any>, kind: &str) -> Result<Vec<KeyId>> {
    let mut result = Vec::new();
    for query in [
        "SELECT sealed_client AS blob FROM mcp_oauth_profiles WHERE owner_kind=$1 AND sealed_client IS NOT NULL",
        "SELECT f.sealed_flow AS blob FROM mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id WHERE p.owner_kind=$1",
        "SELECT f.sealed_callback AS blob FROM mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id WHERE p.owner_kind=$1 AND f.sealed_callback IS NOT NULL",
        "SELECT t.sealed_token AS blob FROM mcp_oauth_tokens t JOIN mcp_oauth_profiles p ON p.id=t.profile_id WHERE p.owner_kind=$1",
        "SELECT e.sealed_response AS blob FROM mcp_oauth_exchanges e JOIN mcp_oauth_profiles p ON p.id=e.profile_id WHERE p.owner_kind=$1 AND e.sealed_response IS NOT NULL",
        "SELECT sealed_ack AS blob FROM mcp_async_tasks WHERE owner_kind=$1",
        "SELECT sealed_checkpoint AS blob FROM mcp_async_tasks WHERE owner_kind=$1 AND sealed_checkpoint IS NOT NULL",
        "SELECT sealed_final_result AS blob FROM mcp_async_tasks WHERE owner_kind=$1 AND sealed_final_result IS NOT NULL",
        "SELECT o.sealed_response AS blob FROM mcp_async_task_operations o JOIN mcp_async_tasks t ON t.id=o.task_record_id WHERE t.owner_kind=$1 AND o.sealed_response IS NOT NULL",
    ] {
        let bounded_query=format!("{query} LIMIT 100001");
        let rows = sqlx::query(&bounded_query)
            .bind(kind)
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
        if rows.len() > 100_000 {
            return Err(Error::BudgetExceeded);
        }
        for row in rows {
            let blob: SealedMcpBlob =
                parse(&row.try_get::<String, _>("blob").map_err(storage_error)?)?;
            blob.validate().map_err(|_| Error::Forbidden)?;
            if !result.contains(&blob.key_id()) {
                result.push(blob.key_id());
            }
        }
    }
    for row in sqlx::query("SELECT checkpoint_key_id FROM mcp_async_tasks WHERE owner_kind=$1 AND sealed_checkpoint IS NOT NULL LIMIT 100001").bind(kind).fetch_all(&mut **tx).await.map_err(storage_error)? {
        let key:KeyId=parse(&row.try_get::<String,_>("checkpoint_key_id").map_err(storage_error)?)?;key.validate().map_err(|_|Error::Forbidden)?;if !result.contains(&key){result.push(key);}
    }
    // Configuration ACKs retain their original fingerprint key, separately from
    // any subsequently rotated encryption key.
    let rows=sqlx::query("SELECT DISTINCT request_key_id FROM mcp_oauth_configuration_operations WHERE owner_kind=$1 LIMIT 100001").bind(kind).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    for row in rows {
        let key: KeyId = parse(
            &row.try_get::<String, _>("request_key_id")
                .map_err(storage_error)?,
        )?;
        key.validate().map_err(|_| Error::Forbidden)?;
        if !result.contains(&key) {
            result.push(key);
        }
    }
    for key in crate::mcp_interactions::required_keys(tx,kind).await? { if !result.contains(&key){result.push(key);} }
    Ok(result)
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
    kind: &str,
) -> Result<u64> {
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_oauth_exchanges e JOIN mcp_oauth_profiles p ON p.id=e.profile_id WHERE p.owner_kind=$1 AND e.state IN('admitted','received')").bind(kind).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let tasks:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_async_task_operations o JOIN mcp_async_tasks t ON t.id=o.task_record_id WHERE t.owner_kind=$1 AND o.state IN('admitted','received')").bind(kind).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 0 || tasks != 0 {
        return Err(Error::Conflict);
    }
    let mut count = 0u64;
    for (table, column, query, profile_scope, callback, checkpoint) in [
        (
            "mcp_oauth_profiles",
            "sealed_client",
            "SELECT id,revision,secret_scope,sealed_client AS blob FROM mcp_oauth_profiles WHERE owner_kind=$1 AND sealed_client IS NOT NULL",
            true,
            false,
            false,
        ),
        (
            "mcp_oauth_flows",
            "sealed_flow",
            "SELECT f.id,f.profile_revision AS revision,f.secret_scope,f.sealed_flow AS blob FROM mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id WHERE p.owner_kind=$1",
            false,
            false,
            false,
        ),
        (
            "mcp_oauth_flows",
            "sealed_callback",
            "SELECT f.id,f.profile_revision AS revision,f.secret_scope,f.sealed_callback AS blob FROM mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id WHERE p.owner_kind=$1 AND f.sealed_callback IS NOT NULL",
            false,
            true,
            false,
        ),
        (
            "mcp_oauth_tokens",
            "sealed_token",
            "SELECT t.id,t.revision,t.secret_scope,t.sealed_token AS blob FROM mcp_oauth_tokens t JOIN mcp_oauth_profiles p ON p.id=t.profile_id WHERE p.owner_kind=$1",
            false,
            false,
            false,
        ),
        (
            "mcp_oauth_exchanges",
            "sealed_response",
            "SELECT e.id,e.profile_revision AS revision,e.secret_scope,e.sealed_response AS blob FROM mcp_oauth_exchanges e JOIN mcp_oauth_profiles p ON p.id=e.profile_id WHERE p.owner_kind=$1 AND e.sealed_response IS NOT NULL",
            false,
            false,
            false,
        ),
        (
            "mcp_async_tasks",
            "sealed_ack",
            "SELECT id,revision,secret_scope,sealed_ack AS blob FROM mcp_async_tasks WHERE owner_kind=$1",
            false,
            false,
            false,
        ),
        (
            "mcp_async_tasks",
            "sealed_checkpoint",
            "SELECT id,revision,run_id,project_id,server_id,server_revision,checkpoint_key_id,secret_scope,sealed_checkpoint AS blob FROM mcp_async_tasks WHERE owner_kind=$1 AND sealed_checkpoint IS NOT NULL",
            false,
            false,
            true,
        ),
        (
            "mcp_async_tasks",
            "sealed_final_result",
            "SELECT id,revision,final_result_scope AS secret_scope,sealed_final_result AS blob FROM mcp_async_tasks WHERE owner_kind=$1 AND sealed_final_result IS NOT NULL",
            false,
            false,
            false,
        ),
        (
            "mcp_async_task_operations",
            "sealed_response",
            "SELECT o.id,o.expected_revision AS revision,o.secret_scope,o.sealed_response AS blob FROM mcp_async_task_operations o JOIN mcp_async_tasks t ON t.id=o.task_record_id WHERE t.owner_kind=$1 AND o.sealed_response IS NOT NULL",
            false,
            false,
            false,
        ),
    ] {
        let bounded_query=format!("{query} LIMIT 100001");
        let rows = sqlx::query(&bounded_query)
            .bind(kind)
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
        if rows.len() > 100_000 {
            return Err(Error::BudgetExceeded);
        }
        for row in rows {
            let id: String = row.try_get("id").map_err(storage_error)?;
            let original: String = row.try_get("blob").map_err(storage_error)?;
            let blob: SealedMcpBlob = parse(&original)?;
            let raw_scope: String = row.try_get("secret_scope").map_err(storage_error)?;
            let mut scope: McpSecretScope = if profile_scope {
                let value: Value = parse(&raw_scope)?;
                serde_json::from_value(value["scope"].clone()).map_err(storage_error)?
            } else {
                parse(&raw_scope)?
            };
            if callback {
                scope.purpose = "oauth_callback".into();
            }
            if checkpoint {
                scope.purpose = "task_checkpoint".into();
                scope.revision = 1;
            }
            if scope.owner_kind != kind || blob.key_id().instance != target.instance {
                return Err(Error::Forbidden);
            }
            if checkpoint {
                let original_locator: String =
                    row.try_get("checkpoint_key_id").map_err(storage_error)?;
                let inner_key: KeyId = parse(&original_locator)?;
                let plaintext =
                    open_mcp_blob(keys, &scope, &blob).map_err(|_| Error::NativeKeyUnavailable)?;
                let value: Value = serde_json::from_slice(&plaintext).map_err(storage_error)?;
                let original_scope: cowork_mcp::CheckpointScope =
                    serde_json::from_value(value["scope"].clone()).map_err(storage_error)?;
                let original_session: cowork_mcp::SealedSession =
                    serde_json::from_value(value["sealed"].clone()).map_err(storage_error)?;
                if inner_key.instance != target.instance
                    || inner_key.cipher_id().map_err(|_| Error::Forbidden)?
                        != original_session.key_id()
                    || original_scope.run_id
                        != row.try_get::<String, _>("run_id").map_err(storage_error)?
                    || original_scope.project_id
                        != row
                            .try_get::<String, _>("project_id")
                            .map_err(storage_error)?
                    || original_scope.server_id
                        != row
                            .try_get::<String, _>("server_id")
                            .map_err(storage_error)?
                    || original_scope.server_revision
                        != row
                            .try_get::<i64, _>("server_revision")
                            .map_err(storage_error)?
                {
                    return Err(Error::Forbidden);
                }
                let original_cipher = cowork_mcp::SessionCipher::new(
                    original_session.key_id(),
                    keys.load(&inner_key)
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                )
                .map_err(|_| Error::NativeKeyUnavailable)?;
                let target_cipher = cowork_mcp::SessionCipher::new(
                    &target.cipher_id().map_err(|_| Error::Forbidden)?,
                    keys.load(&target)
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                )
                .map_err(|_| Error::NativeKeyUnavailable)?;
                let rewritten_session = original_cipher
                    .rewrap(&original_session, &original_scope, &target_cipher)
                    .map_err(|_| Error::NativeKeyUnavailable)?;
                if blob.key_id() == target && inner_key == target {
                    continue;
                }
                let new = seal_mcp_blob(
                    keys,
                    target,
                    &scope,
                    &Zeroizing::new(
                        serde_json::to_vec(
                            &json!({"scope":original_scope,"sealed":rewritten_session}),
                        )
                        .map_err(storage_error)?,
                    ),
                )
                .map_err(|_| Error::NativeKeyUnavailable)?;
                if sqlx::query("UPDATE mcp_async_tasks SET sealed_checkpoint=$1,checkpoint_key_id=$2 WHERE id=$3 AND sealed_checkpoint=$4 AND checkpoint_key_id=$5").bind(json_string(&new)?).bind(json_string(&target)?).bind(&id).bind(original).bind(original_locator).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
                count = count.checked_add(1).ok_or(Error::BudgetExceeded)?;
                continue;
            }
            let rewritten = cowork_secrets::rewrap_mcp_blob(keys, &scope, &blob, target)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            if blob.key_id() == target {
                continue;
            }
            let update = format!("UPDATE {table} SET {column}=$1 WHERE id=$2 AND {column}=$3");
            if sqlx::query(&update)
                .bind(json_string(&rewritten)?)
                .bind(id)
                .bind(original)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?
                .rows_affected()
                != 1
            {
                return Err(Error::Conflict);
            }
            count = count.checked_add(1).ok_or(Error::BudgetExceeded)?;
        }
    }
    count=count.checked_add(crate::mcp_interactions::rewrap(tx,keys,target,kind).await?).ok_or(Error::BudgetExceeded)?;
    Ok(count)
}

pub(crate) async fn ciphertext_counts(
    tx: &mut Transaction<'_, Any>,
    kind: &str,
) -> Result<(u64, u64)> {
    let mut rows = 0u64;
    let mut bytes = 0u64;
    for (table, alias, owner, columns) in [
        (
            "mcp_oauth_profiles p",
            "p",
            "p.owner_kind",
            &["sealed_client"][..],
        ),
        (
            "mcp_oauth_flows f JOIN mcp_oauth_profiles p ON p.id=f.profile_id",
            "f",
            "p.owner_kind",
            &["sealed_flow", "sealed_callback"][..],
        ),
        (
            "mcp_oauth_tokens t JOIN mcp_oauth_profiles p ON p.id=t.profile_id",
            "t",
            "p.owner_kind",
            &["sealed_token"][..],
        ),
        (
            "mcp_oauth_exchanges e JOIN mcp_oauth_profiles p ON p.id=e.profile_id",
            "e",
            "p.owner_kind",
            &["sealed_response"][..],
        ),
        (
            "mcp_async_tasks t",
            "t",
            "t.owner_kind",
            &["sealed_ack", "sealed_checkpoint", "sealed_final_result"][..],
        ),
        (
            "mcp_async_task_operations o JOIN mcp_async_tasks t ON t.id=o.task_record_id",
            "o",
            "t.owner_kind",
            &["sealed_response"][..],
        ),
        (
            "mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id",
            "i", "t.owner_kind",
            &["sealed_request","sealed_decision","sealed_model_intent","sealed_model_response","sealed_response"][..],
        ),
        (
            "mcp_task_interaction_decisions d JOIN mcp_task_interactions i ON i.id=d.interaction_id JOIN mcp_async_tasks t ON t.id=i.task_record_id",
            "d", "t.owner_kind", &["sealed_decision"][..],
        ),
    ] {
        let sizes = columns
            .iter()
            .map(|column| match tx.backend_name() {
                "SQLite" => Ok(format!(
                    "LENGTH(CAST(COALESCE({alias}.{column},'') AS BLOB))"
                )),
                "PostgreSQL" => Ok(format!("OCTET_LENGTH(COALESCE({alias}.{column},''))")),
                _ => Err(Error::Conflict),
            })
            .collect::<Result<Vec<_>>>()?;
        let query = format!(
            "SELECT COUNT(*) AS n,CAST(COALESCE(SUM({}),0) AS BIGINT) AS bytes FROM {table} WHERE {owner}=$1",
            sizes.join("+")
        );
        let row = sqlx::query(&query)
            .bind(kind)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        rows = rows
            .checked_add(
                u64::try_from(row.try_get::<i64, _>("n").map_err(storage_error)?)
                    .map_err(|_| Error::Forbidden)?,
            )
            .ok_or(Error::BudgetExceeded)?;
        bytes = bytes
            .checked_add(
                u64::try_from(row.try_get::<i64, _>("bytes").map_err(storage_error)?)
                    .map_err(|_| Error::Forbidden)?,
            )
            .ok_or(Error::BudgetExceeded)?;
        if rows > 100_000 || bytes > 512 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
    }
    Ok((rows, bytes))
}
