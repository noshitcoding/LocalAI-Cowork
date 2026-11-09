use super::*;
use cowork_secrets::{
    KeyId, KeyProof, KeyStore, NativeKeyManager, NativeKeyProvider, load_verified_key, prove_key,
    verify_key,
};
use cowork_storage::{NativeKeyAdmission, NativeKeyOperation};
use zeroize::Zeroizing;

static NATIVE_OPERATIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
pub(crate) async fn native_call<T, F>(operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> cowork_secrets::Result<T> + Send + 'static,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let slot = tokio::time::timeout_at(deadline, NATIVE_OPERATIONS.acquire())
        .await
        .map_err(|_| Error::NativeKeyUnavailable)?
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let task = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        operation()
    });
    tokio::time::timeout_at(deadline, task)
        .await
        .map_err(|_| Error::NativeKeyUnavailable)?
        .map_err(|_| Error::NativeKeyUnavailable)?
        .map_err(|_| Error::NativeKeyUnavailable)
}
impl Runtime {
    pub(crate) fn native_key_provider(&self) -> Result<Arc<dyn NativeKeyProvider>> {
        // Current native setup belongs to the single-owner local daemon. The
        // PostgreSQL storage contract is shared, not an enterprise key authority.
        if !self.store.is_local_sqlite() {
            return Err(Error::Forbidden);
        }
        self.native_keys.clone().ok_or(Error::Forbidden)
    }
    pub async fn native_secrets_status(&self) -> Result<NativeSecretsStatus> {
        let provider = self.native_key_provider()?;
        let Some(state) = self.store.native_secret_state().await? else {
            return Ok(NativeSecretsStatus {
                revision: 0,
                configured: false,
                key_available: false,
                pending_operation_id: None,
            });
        };
        let configured = state.active.is_some();
        let key_available = if let Some(proof) = state.active {
            native_call(move || {
                let manager = provider.open(proof.key.instance)?;
                verify_key(manager.as_ref(), &proof)
            })
            .await
            .is_ok()
        } else {
            false
        };
        Ok(NativeSecretsStatus {
            revision: state.revision,
            configured,
            key_available,
            pending_operation_id: state.pending_operation,
        })
    }
    async fn native_secrets_receipt(
        &self,
        operation: NativeKeyOperation,
    ) -> Result<NativeSecretsReceipt> {
        Ok(NativeSecretsReceipt {
            operation_id: operation.id,
            operation_state: operation.state,
            store: self.native_secrets_status().await?,
        })
    }
    pub async fn native_secrets_request(&self, key: &str) -> Result<NativeSecretsRequestLookup> {
        self.native_key_provider()?;
        let receipt = match self.store.native_key_request(key).await? {
            Some(operation) => Some(self.native_secrets_receipt(operation).await?),
            None => None,
        };
        Ok(NativeSecretsRequestLookup { receipt })
    }
    pub async fn setup_native_secrets(
        &self,
        input: &NativeSecretsSetup,
    ) -> Result<NativeSecretsReceipt> {
        let provider = self.native_key_provider()?;
        let permit = match self.store.begin_native_key_setup(input).await? {
            NativeKeyAdmission::Existing(operation) => {
                return self.native_secrets_receipt(operation).await;
            }
            NativeKeyAdmission::Create(permit) => permit,
        };
        let operation = permit.into_operation();
        let key = operation.key;
        let guard = self.store.begin_native_key_creation(&operation).await?;
        let (proof, guard) = native_call(move || {
            let manager = provider.open(key.instance)?;
            manager.create(&key)?;
            Ok((prove_key(manager.as_ref(), &key)?, guard))
        })
        .await
        .map_err(|_| Error::UncertainEffect)?;
        guard.finish().await?;
        let operation = self
            .store
            .finish_native_key_setup(
                &NativeSecretsResolution {
                    operation_id: operation.id.clone(),
                    base_revision: operation.reserved_revision(),
                },
                Some(&proof),
            )
            .await?;
        self.native_secrets_receipt(operation).await
    }
    pub async fn reconcile_native_secrets(
        &self,
        input: &NativeSecretsResolution,
    ) -> Result<NativeSecretsReceipt> {
        let provider = self.native_key_provider()?;
        let operation = self.store.native_key_operation(&input.operation_id).await?;
        if input.base_revision != operation.reserved_revision() {
            return Err(Error::Conflict);
        }
        if operation.state != NativeSecretsSetupState::Pending {
            if operation.state != NativeSecretsSetupState::Activated {
                return Err(Error::Conflict);
            }
            return self.native_secrets_receipt(operation).await;
        }
        let key = operation.key;
        // Reconcile reads the reserved candidate. It never calls create.
        let proof = native_call(move || {
            let manager = provider.open(key.instance)?;
            prove_key(manager.as_ref(), &key)
        })
        .await?;
        let operation = self
            .store
            .finish_native_key_setup(input, Some(&proof))
            .await?;
        self.native_secrets_receipt(operation).await
    }
    pub async fn abandon_native_secrets(
        &self,
        input: &NativeSecretsResolution,
    ) -> Result<NativeSecretsReceipt> {
        self.native_key_provider()?;
        let operation = self.store.finish_native_key_setup(input, None).await?;
        // Do not delete an OS key here: a timed-out creator may still be running.
        self.native_secrets_receipt(operation).await
    }
    /// Uses persisted native identity after restart. Opening performs no setup.
    pub async fn dispatch_configured_mcp_tool(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        stop: &CancellationToken,
    ) -> Result<McpAttempt> {
        // Single-owner server runtimes have explicitly installed server custody.
        // This method is never an actor-session credential adapter.
        if self.native_keys.is_none() {
            let access = self
                .personal_provider_keys
                .clone()
                .ok_or(Error::NativeKeyUnavailable)?;
            return self
                .dispatch_custodied_mcp_tool(
                    run,
                    token,
                    tool,
                    &McpSecretAccess {
                        keys: access.keys,
                        active_key: access.key_id,
                        limits: cowork_mcp::Limits::default(),
                    },
                    stop,
                )
                .await;
        }
        let provider = self.native_key_provider()?;
        let state = self
            .store
            .native_secret_state()
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let active = state.active.ok_or(Error::NativeKeyUnavailable)?;
        let manager = provider
            .open(state.instance)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let active_key = active.key;
        let access = McpSecretAccess {
            keys: Arc::new(VerifiedNativeKeys { manager, active }),
            active_key,
            limits: cowork_mcp::Limits::default(),
        };
        self.dispatch_custodied_mcp_tool(run, token, tool, &access, stop)
            .await
    }
}
pub(crate) struct VerifiedNativeKeys {
    pub(crate) manager: Arc<dyn NativeKeyManager>,
    pub(crate) active: KeyProof,
}
impl KeyStore for VerifiedNativeKeys {
    fn load(&self, key: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if key.instance != self.active.key.instance {
            return Err(cowork_secrets::Error::Invalid);
        }
        if key == &self.active.key {
            load_verified_key(self.manager.as_ref(), &self.active)
        } else {
            self.manager.load(key)
        }
    }
}
