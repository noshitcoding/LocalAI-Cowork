use super::native_secrets::{VerifiedNativeKeys, native_call};
use super::*;
use std::collections::BTreeMap;
use zeroize::Zeroizing;

fn equal_secret(first: &str, second: &str) -> bool {
    let comparison = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"cowork-mcp-ack-comparison");
    let tag = ring::hmac::sign(&comparison, first.as_bytes());
    ring::hmac::verify(&comparison, second.as_bytes(), tag.as_ref()).is_ok()
}
impl Runtime {
    /// An immutable connector revision has exactly one atomic encrypted config.
    /// Concurrent/lost-ACK retry compares plaintext under custody; no secret hash
    /// or altered revision is published for the same operation.
    pub async fn save_connector_environment(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        value: &str,
        native: bool,
    ) -> Result<BTreeMap<String, i64>> {
        cowork_toolbox::validate_stdio_environment(&BTreeMap::from([(
            "CONNECTOR_CONFIG".into(),
            value.into(),
        )]))
        .map_err(|_| {
            Error::Validation(
                "Connector URL and key exceed the encrypted configuration limit".into(),
            )
        })?;
        let reference = environment_reference(server, "CONNECTOR_CONFIG", native);
        let encoded = Zeroizing::new(format!(
            "00{}",
            value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
        let (_, revision) = self
            .save_custodied_mcp_value(
                project,
                server,
                endpoint,
                &reference,
                encoded.to_string(),
                0,
            )
            .await?;
        Ok(BTreeMap::from([("CONNECTOR_CONFIG".into(), revision)]))
    }
    pub fn server_owner_mcp_available(&self) -> bool {
        self.native_keys.is_none() && self.personal_provider_keys.is_some()
    }
    /// Admission verifies the selected host's custody before accepting work.
    /// Server owner keys never stand in for a native factory or native secret.
    pub(crate) async fn validate_owner_mcp_bindings(
        &self,
        project: &str,
        bindings: &[McpToolBinding],
    ) -> Result<()> {
        if bindings.is_empty() {
            return Ok(());
        }
        if !self.server_owner_mcp_available() && !self.native_secrets_status().await?.key_available
        {
            return Err(Error::NativeKeyUnavailable);
        }
        for binding in bindings {
            if let Some(factory) = self.store.native_mcp_factory(&binding.server_id).await?
                && (factory.project_id != project
                    || !factory.enabled
                    || factory.endpoint != binding.endpoint
                    || factory.native_only != self.native_keys.is_some())
            {
                return Err(Error::Forbidden);
            }
            self.workflow_mcp_bearer(project, binding).await?;
        }
        Ok(())
    }
    pub async fn save_native_mcp_bearer(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        bearer: String,
        expected: i64,
    ) -> Result<(String, i64)> {
        self.save_custodied_mcp_value(
            project,
            server,
            endpoint,
            &format!("owner-mcp:{server}"),
            bearer,
            expected,
        )
        .await
    }
    pub async fn save_server_mcp_bearer(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        bearer: String,
        expected: i64,
    ) -> Result<(String, i64)> {
        if !self.server_owner_mcp_available() {
            return Err(Error::Forbidden);
        }
        self.save_custodied_mcp_value(
            project,
            server,
            endpoint,
            &format!("server-mcp:{server}"),
            bearer,
            expected,
        )
        .await
    }
    async fn save_custodied_mcp_value(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        reference: &str,
        bearer: String,
        expected: i64,
    ) -> Result<(String, i64)> {
        if expected < 0 {
            return Err(Error::Conflict);
        }
        uuid::Uuid::parse_str(server).map_err(|_| Error::Forbidden)?;
        self.store
            .projects()
            .await?
            .into_iter()
            .find(|p| p.id == project)
            .ok_or(Error::NotFound)?;
        let endpoint = cowork_mcp::Endpoint::new(endpoint, Some(&bearer))
            .map_err(|_| Error::Forbidden)?
            .url()
            .to_owned();
        let scope = cowork_secrets::CredentialScope {
            project_id: project.into(),
            server_id: server.into(),
            reference: reference.into(),
            revision: expected.checked_add(1).ok_or(Error::Conflict)?,
            endpoint,
        };
        let bearer = Zeroizing::new(bearer);
        let existing = match self.store.mcp_credential(project, &scope.reference).await {
            Ok(value) => Some(value),
            Err(Error::NotFound) => None,
            Err(error) => return Err(error),
        };
        if let Some(existing) = existing {
            if existing.revoked
                || existing.scope.server_id != server
                || existing.scope.endpoint != scope.endpoint
            {
                return Err(Error::Forbidden);
            }
            if existing.revision == scope.revision {
                let saved = self
                    .native_mcp_bearer(
                        project,
                        server,
                        &scope.endpoint,
                        &scope.reference,
                        existing.revision,
                    )
                    .await?;
                if !equal_secret(&saved, &bearer) {
                    return Err(Error::Conflict);
                }
                return Ok((existing.scope.reference, existing.revision));
            }
            if existing.revision != expected {
                return Err(Error::Conflict);
            }
        }
        let private_scope = scope.clone();
        let sealed = if reference.starts_with("server-mcp") {
            if !self.server_owner_mcp_available() {
                return Err(Error::Forbidden);
            }
            let access = self
                .personal_provider_keys
                .clone()
                .ok_or(Error::NativeKeyUnavailable)?;
            native_call(move || {
                cowork_secrets::seal_bearer(
                    access.keys.as_ref(),
                    access.key_id,
                    &private_scope,
                    &bearer,
                )
            })
            .await?
        } else {
            let provider = self.native_key_provider()?;
            let state = self
                .store
                .native_secret_state()
                .await?
                .ok_or(Error::NativeKeyUnavailable)?;
            let active = state.active.ok_or(Error::NativeKeyUnavailable)?;
            let active_key = active.key;
            native_call(move || {
                let manager = provider.open(state.instance)?;
                let keys = VerifiedNativeKeys { manager, active };
                cowork_secrets::seal_bearer(&keys, active_key, &private_scope, &bearer)
            })
            .await?
        };
        let saved = self
            .store
            .put_mcp_credential(&scope, &sealed, expected)
            .await?;
        Ok((saved.scope.reference, saved.revision))
    }
    pub async fn save_mcp_stdio_environment(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        environment: &BTreeMap<String, String>,
        native: bool,
    ) -> Result<BTreeMap<String, i64>> {
        cowork_toolbox::validate_stdio_environment(environment)
            .map_err(|_| Error::Validation("Invalid MCP environment".into()))?;
        let mut versions = BTreeMap::new();
        for (name, value) in environment {
            let reference = environment_reference(server, name, native);
            let encoded = Zeroizing::new(format!(
                "00{}",
                value
                    .as_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ));
            let current = match self.store.mcp_credential(project, &reference).await {
                Ok(value) => Some(value),
                Err(Error::NotFound) => None,
                Err(error) => return Err(error),
            };
            let expected = if let Some(saved) = &current {
                let old = self
                    .native_mcp_bearer(project, server, endpoint, &reference, saved.revision)
                    .await?;
                if equal_secret(&old, &encoded) {
                    versions.insert(name.clone(), saved.revision);
                    continue;
                }
                saved.revision
            } else {
                0
            };
            let (_, revision) = self
                .save_custodied_mcp_value(
                    project,
                    server,
                    endpoint,
                    &reference,
                    encoded.to_string(),
                    expected,
                )
                .await?;
            versions.insert(name.clone(), revision);
        }
        Ok(versions)
    }
    pub async fn mcp_stdio_environment(
        &self,
        factory: &cowork_storage::NativeMcpFactory,
    ) -> Result<BTreeMap<String, String>> {
        let mut environment = BTreeMap::new();
        for (name, revision) in &factory.environment {
            let reference = environment_reference(&factory.server_id, name, factory.native_only);
            let encoded = self
                .native_mcp_bearer(
                    &factory.project_id,
                    &factory.server_id,
                    &factory.endpoint,
                    &reference,
                    *revision,
                )
                .await?;
            let value = encoded.strip_prefix("00").ok_or(Error::Forbidden)?;
            if value.len() % 2 != 0 {
                return Err(Error::Forbidden);
            }
            let bytes = (0..value.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| Error::Forbidden))
                .collect::<Result<Vec<_>>>()?;
            environment.insert(
                name.clone(),
                String::from_utf8(bytes).map_err(|_| Error::Forbidden)?,
            );
        }
        cowork_toolbox::validate_stdio_environment(&environment).map_err(|_| Error::Forbidden)?;
        Ok(environment)
    }
    pub(crate) async fn workflow_mcp_bearer(
        &self,
        project: &str,
        binding: &McpToolBinding,
    ) -> Result<Option<Zeroizing<String>>> {
        let server = self.store.mcp_server(project, &binding.server_id).await?;
        if !server.bindings()?.contains(binding) {
            return Err(Error::Forbidden);
        }
        if let Some(reference) = server
            .configuration
            .credential_reference
            .as_deref()
            .filter(|v| v.starts_with("oauth:"))
        {
            return self
                .native_mcp_oauth_bearer(
                    project,
                    &server.id,
                    &server.configuration.endpoint,
                    reference,
                    server.configuration.credential_revision,
                )
                .await
                .map(Some);
        }
        let Some(credential) = self.store.mcp_server_credential(&server).await? else {
            return Ok(None);
        };
        self.native_mcp_bearer(
            project,
            &server.id,
            &server.configuration.endpoint,
            &credential.scope.reference,
            credential.revision,
        )
        .await
        .map(Some)
    }
    /// Namespace selects explicit custody; unavailable native keys never fall back to server keys.
    pub async fn native_mcp_bearer(
        &self,
        project: &str,
        server: &str,
        endpoint: &str,
        reference: &str,
        revision: i64,
    ) -> Result<Zeroizing<String>> {
        if reference.starts_with("oauth:") {
            return self
                .native_mcp_oauth_bearer(project, server, endpoint, reference, revision)
                .await;
        }
        let credential = self.store.mcp_credential(project, reference).await?;
        let normalized = cowork_mcp::Endpoint::new(endpoint, None)
            .map_err(|_| Error::Forbidden)?
            .url()
            .to_owned();
        if credential.revoked
            || credential.revision != revision
            || credential.scope.server_id != server
            || credential.scope.endpoint != normalized
        {
            return Err(Error::Forbidden);
        }
        if reference.starts_with("server-mcp") {
            if !self.server_owner_mcp_available() {
                return Err(Error::Forbidden);
            }
            let access = self
                .personal_provider_keys
                .clone()
                .ok_or(Error::NativeKeyUnavailable)?;
            return native_call(move || {
                let sealed = credential
                    .sealed
                    .as_ref()
                    .ok_or(cowork_secrets::Error::Invalid)?;
                cowork_secrets::open_bearer(access.keys.as_ref(), &credential.scope, sealed)
            })
            .await;
        }
        let provider = self.native_key_provider()?;
        let state = self
            .store
            .native_secret_state()
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let active = state.active.ok_or(Error::NativeKeyUnavailable)?;
        native_call(move || {
            let manager = provider.open(state.instance)?;
            let keys = VerifiedNativeKeys { manager, active };
            let sealed = credential
                .sealed
                .as_ref()
                .ok_or(cowork_secrets::Error::Invalid)?;
            cowork_secrets::open_bearer(&keys, &credential.scope, sealed)
        })
        .await
    }
}
fn environment_reference(server: &str, name: &str, native: bool) -> String {
    format!(
        "{}-mcp-env:{server}:{name}",
        if native { "native" } else { "server" }
    )
}
