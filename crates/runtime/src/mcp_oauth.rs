//! Native OAuth controller. Credential custody belongs to the native key owner;
//! browser callbacks only capture evidence and cannot dispatch a token exchange.
use super::native_secrets::{VerifiedNativeKeys, native_call};
use super::*;
use cowork_mcp::oauth::{OAuthNetwork, TokenRequest};
use cowork_secrets::{KeyId, open_mcp_blob};
use cowork_storage::mcp_oauth::{
    McpOAuthCallback, McpOAuthExchangeAdmission, McpOAuthFlowSecret, McpOAuthOwner,
    McpOAuthTokenSecret,
};
use zeroize::Zeroizing;
impl Runtime {
    pub(crate) async fn native_mcp_keys(&self) -> Result<(Arc<VerifiedNativeKeys>, KeyId)> {
        let provider = self.native_key_provider()?;
        let state = self
            .store
            .native_secret_state()
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        if state.pending_operation.is_some() {
            return Err(Error::NativeKeyUnavailable);
        }
        let active = state.active.ok_or(Error::NativeKeyUnavailable)?;
        let key = active.key;
        let keys = native_call(move || {
            let manager = provider.open(state.instance)?;
            cowork_secrets::verify_key(manager.as_ref(), &active)?;
            Ok(Arc::new(VerifiedNativeKeys { manager, active }))
        })
        .await?;
        Ok((keys, key))
    }
    pub async fn discover_native_mcp_oauth(
        &self,
        project: &str,
        input: &McpOAuthDiscovery,
    ) -> Result<McpOAuthMetadata> {
        let (keys, key) = self.native_mcp_keys().await?;
        let owner = McpOAuthOwner::Native { key };
        // No metadata IO precedes actual current server/project authority.
        let server = self.store.mcp_server(project, &input.server_id).await?;
        if server.revision != input.server_revision
            || server.configuration.endpoint != input.resource
        {
            return Err(Error::Conflict);
        }
        let metadata = cowork_mcp::oauth::discover(&input.resource, OAuthNetwork::NativeOwner)
            .await
            .map_err(|_| Error::McpSessionUnavailable)?;
        let current = self.store.mcp_server(project, &input.server_id).await?;
        if current != server {
            return Err(Error::Conflict);
        }
        // An existing profile is rechecked rather than adopted from discovery.
        let _ = owner;
        let _ = keys;
        Ok(McpOAuthMetadata{resource:metadata.resource,resource_metadata_uri:metadata.metadata_uri,authorization_servers:metadata.authorization_servers.into_iter().map(|m|json!({"issuer":m.issuer,"authorization_endpoint":m.authorization_endpoint,"token_endpoint":m.token_endpoint,"scopes_supported":m.scopes_supported,"token_endpoint_auth_methods_supported":m.token_endpoint_auth_methods_supported})).collect(),scopes_supported:metadata.scopes_supported,suggested_scopes:metadata.suggested_scopes,sha256:metadata.sha256})
    }
    pub async fn configure_native_mcp_oauth(
        &self,
        project: &str,
        input: &ConfigureMcpOAuth,
    ) -> Result<McpOAuthProfile> {
        if input.configuration.redirect_uri != "http://127.0.0.1:43821/mcp/oauth/callback"
            || input.configuration.shared_team_consent
        {
            return Err(Error::Forbidden);
        }
        let (keys, key) = self.native_mcp_keys().await?;
        let owner = McpOAuthOwner::Native { key };
        if let Some(receipt) = self
            .store
            .mcp_oauth_configuration_receipt(&owner, project, input, keys.as_ref())
            .await?
        {
            return Ok(receipt);
        }
        let before = self.store.mcp_server(project, &input.server_id).await?;
        if before.configuration.endpoint != input.configuration.resource {
            return Err(Error::Forbidden);
        }
        let metadata =
            cowork_mcp::oauth::discover(&input.configuration.resource, OAuthNetwork::NativeOwner)
                .await
                .map_err(|_| Error::McpSessionUnavailable)?;
        self.store
            .configure_mcp_oauth(&owner, project, input, &metadata, keys.as_ref())
            .await
    }
    pub async fn native_mcp_oauth_profiles(&self, project: &str) -> Result<Vec<McpOAuthProfile>> {
        let (keys, key) = self.native_mcp_keys().await?;
        self.store
            .mcp_oauth_profiles(&McpOAuthOwner::Native { key }, project, keys.as_ref())
            .await
    }
    pub async fn start_native_mcp_oauth(
        &self,
        profile: &str,
        input: &McpOAuthRequest,
    ) -> Result<McpOAuthFlow> {
        let (keys, key) = self.native_mcp_keys().await?;
        self.store
            .start_mcp_oauth(
                &McpOAuthOwner::Native { key },
                profile,
                input,
                keys.as_ref(),
            )
            .await
    }
    pub async fn native_mcp_oauth_flow(&self, flow: &str) -> Result<McpOAuthFlow> {
        let (keys, key) = self.native_mcp_keys().await?;
        Ok(self
            .store
            .mcp_oauth_flow(&McpOAuthOwner::Native { key }, flow, keys.as_ref())
            .await?
            .view)
    }
    pub async fn native_mcp_oauth_callback(&self, callback: &McpOAuthCallback) -> Result<()> {
        let (keys, key) = self.native_mcp_keys().await?;
        let flow = self
            .store
            .native_mcp_oauth_callback_flow(&callback.state, key)
            .await?;
        self.store
            .capture_mcp_oauth_callback(
                &McpOAuthOwner::Native { key },
                &flow,
                callback,
                keys.as_ref(),
            )
            .await?;
        Ok(())
    }
    pub async fn complete_native_mcp_oauth(&self, flow: &str) -> Result<Value> {
        let (keys, key) = self.native_mcp_keys().await?;
        let owner = McpOAuthOwner::Native { key };
        let f = self
            .store
            .mcp_oauth_flow(&owner, flow, keys.as_ref())
            .await?;
        let p = self
            .store
            .mcp_oauth_profile(&owner, &f.view.profile_id, keys.as_ref())
            .await?;
        let secret: McpOAuthFlowSecret = serde_json::from_slice(
            &open_mcp_blob(keys.as_ref(), &f.scope, &f.sealed)
                .map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(|_| Error::Forbidden)?;
        let mut callback_scope = f.scope.clone();
        callback_scope.purpose = "oauth_callback".into();
        let callback: McpOAuthCallback = serde_json::from_slice(
            &open_mcp_blob(
                keys.as_ref(),
                &callback_scope,
                f.callback.as_ref().ok_or(Error::Conflict)?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(|_| Error::Forbidden)?;
        let client_secret = p
            .sealed_client
            .as_ref()
            .map(|s| {
                open_mcp_blob(keys.as_ref(), &p.client_scope, s)
                    .and_then(|s| {
                        String::from_utf8(s.to_vec())
                            .map(Zeroizing::new)
                            .map_err(|_| cowork_secrets::Error::Invalid)
                    })
                    .map_err(|_| Error::NativeKeyUnavailable)
            })
            .transpose()?;
        let request = TokenRequest::authorization_code(
            &p.authorization,
            &p.view.configuration.client_id,
            client_secret,
            &p.view.configuration.redirect_uri,
            &p.view.configuration.resource,
            Zeroizing::new(callback.code.as_ref().ok_or(Error::Forbidden)?.clone()),
            Zeroizing::new(secret.verifier.clone()),
        )
        .map_err(|_| Error::Forbidden)?;
        self.native_mcp_token_exchange(
            &owner,
            &p.view.id,
            flow,
            "authorization_code",
            request,
            keys.as_ref(),
            key,
        )
        .await
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "native_mcp_token_exchange: Keep original OAuth owner/profile/source/kind, prepared request pins and native key custody explicit"
    )]
    async fn native_mcp_token_exchange(
        &self,
        owner: &McpOAuthOwner<'_>,
        profile: &str,
        source: &str,
        kind: &str,
        request: TokenRequest,
        keys: &VerifiedNativeKeys,
        key: KeyId,
    ) -> Result<Value> {
        let permit = match self
            .store
            .begin_mcp_oauth_exchange(owner, profile, source, kind, request.sha256(), keys)
            .await?
        {
            McpOAuthExchangeAdmission::Original(_) => {
                return self
                    .store
                    .finish_captured_mcp_oauth_exchange(owner, profile, source, kind, keys)
                    .await;
            }
            McpOAuthExchangeAdmission::Dispatch(permit) => permit,
        };
        self.store
            .validate_mcp_oauth_exchange(owner, &permit, keys)
            .await?;
        let response = match request.dispatch(OAuthNetwork::NativeOwner).await {
            Ok(response) => json!({"status":response.status,"body":response.body}),
            Err(_) => return self.store.unknown_mcp_oauth_exchange(&permit).await,
        };
        self.store
            .capture_mcp_oauth_exchange(&permit, &response, keys, key)
            .await?;
        self.store
            .commit_mcp_oauth_exchange(owner, &permit, keys)
            .await
    }
    /// Refresh before admitting a tool; a401 from an admitted call never causes
    /// replay of that call. Unknown token effects require explicit reauthorization.
    pub async fn native_mcp_oauth_bearer(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        reference: &str,
        revision: i64,
    ) -> Result<Zeroizing<String>> {
        let profile = reference.strip_prefix("oauth:").ok_or(Error::Forbidden)?;
        let (keys, key) = self.native_mcp_keys().await?;
        let owner = McpOAuthOwner::Native { key };
        let p = self
            .store
            .mcp_oauth_profile(&owner, profile, keys.as_ref())
            .await?;
        if p.view.project_id != project
            || p.view.server_id != server
            || p.view.configuration.resource != endpoint
            || p.view.revision != revision
        {
            return Err(Error::Forbidden);
        }
        let token = self
            .store
            .mcp_oauth_token(&owner, profile, keys.as_ref())
            .await?;
        let secret: McpOAuthTokenSecret = serde_json::from_slice(
            &open_mcp_blob(keys.as_ref(), &token.scope, &token.sealed)
                .map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(|_| Error::Forbidden)?;
        if token.expires_at <= now() + 30_000 {
            let refresh = secret
                .refresh
                .as_ref()
                .ok_or(Error::McpSessionUnavailable)?;
            let client_secret = p
                .sealed_client
                .as_ref()
                .map(|s| {
                    open_mcp_blob(keys.as_ref(), &p.client_scope, s)
                        .and_then(|s| {
                            String::from_utf8(s.to_vec())
                                .map(Zeroizing::new)
                                .map_err(|_| cowork_secrets::Error::Invalid)
                        })
                        .map_err(|_| Error::NativeKeyUnavailable)
                })
                .transpose()?;
            let request = TokenRequest::refresh(
                &p.authorization,
                &p.view.configuration.client_id,
                client_secret,
                endpoint,
                Zeroizing::new(refresh.clone()),
                &token.scopes,
            )
            .map_err(|_| Error::Forbidden)?;
            let receipt = self
                .native_mcp_token_exchange(
                    &owner,
                    profile,
                    &token.id,
                    "refresh_token",
                    request,
                    keys.as_ref(),
                    key,
                )
                .await?;
            if receipt["state"] != "connected" {
                return Err(Error::McpSessionUnavailable);
            }
            let current = self
                .store
                .mcp_oauth_token(&owner, profile, keys.as_ref())
                .await?;
            let secret: McpOAuthTokenSecret = serde_json::from_slice(
                &open_mcp_blob(keys.as_ref(), &current.scope, &current.sealed)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| Error::Forbidden)?;
            if current.expires_at <= now() {
                return Err(Error::McpSessionUnavailable);
            }
            return Ok(Zeroizing::new(secret.access.clone()));
        }
        Ok(Zeroizing::new(secret.access.clone()))
    }
}
