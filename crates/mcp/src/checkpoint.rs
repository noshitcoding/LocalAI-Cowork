//! Encrypted continuation state, not authorization. Only a current durable Ready
//! connection may be opened. An older authentic ciphertext is not a replay permit.
use super::*;
use ring::{
    aead, hkdf,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckpointScope {
    pub project_id: String,
    pub run_id: String,
    pub server_id: String,
    pub connection_record_id: String,
    pub server_revision: i64,
    pub checkpoint_revision: i64,
    pub configuration_sha256: String,
    pub catalog_sha256: String,
}
impl CheckpointScope {
    fn valid(&self) -> bool {
        [
            &self.project_id,
            &self.run_id,
            &self.server_id,
            &self.connection_record_id,
        ]
        .iter()
        .all(|s| !s.is_empty() && s.len() <= 128 && !s.contains('\0'))
            && self.server_revision > 0
            && self.checkpoint_revision > 0
            && [&self.configuration_sha256, &self.catalog_sha256]
                .iter()
                .all(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
    }
}

/// Ciphertext only. Never include this private database record in public exports.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SealedSession {
    version: u32,
    key_id: String,
    salt: [u8; 32],
    ciphertext: Vec<u8>,
}
impl SealedSession {
    /// Nonsecret locator for the trusted key owner; never a fallback instruction.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !valid_key_id(&self.key_id)
            || !(16..=8192).contains(&self.ciphertext.len())
        {
            return Err(Error::SessionUnavailable);
        }
        Ok(())
    }
}

/// Supplied by the native/server secret owner. No environment lookup, file IO,
/// key generation fallback, Debug, Clone or key serialization is provided here.
pub struct SessionCipher {
    key_id: String,
    master: Zeroizing<[u8; 32]>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlainCheckpoint {
    protocol: String,
    endpoint: String,
    identity: Uuid,
    epoch: u64,
    catalog_sha256: String,
    session_id: Option<Zeroizing<Vec<u8>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_capabilities: Option<TaskCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_task: Option<tasks::PendingTask>,
    #[serde(default, skip_serializing_if = "is_false")]
    task_interactions: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
impl SessionCipher {
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    /// Change encryption custody without initializing or replaying the remote session.
    pub fn rewrap(
        &self,
        sealed: &SealedSession,
        scope: &CheckpointScope,
        target: &SessionCipher,
    ) -> Result<SealedSession> {
        sealed.validate()?;
        if sealed.key_id != self.key_id {
            return Err(Error::SessionUnavailable);
        }
        let mut bytes = Zeroizing::new(sealed.ciphertext.clone());
        let plain = self
            .key(&sealed.salt, &self.aad(scope)?)?
            .open_in_place(
                aead::Nonce::assume_unique_for_key([0; 12]),
                aead::Aad::from(self.aad(scope)?),
                &mut bytes,
            )
            .map_err(|_| Error::SessionUnavailable)?;
        let checkpoint: PlainCheckpoint =
            serde_json::from_slice(plain).map_err(|_| Error::SessionUnavailable)?;
        if !matches!(
            checkpoint.protocol.as_str(),
            PROTOCOL_VERSION | "2025-03-26" | "2025-06-18"
        ) || checkpoint.identity.is_nil()
            || checkpoint.epoch == u64::MAX
        {
            return Err(Error::SessionUnavailable);
        }
        if self.key_id == target.key_id {
            // A no-op custody assertion still authenticates original scope.
            return Ok(sealed.clone());
        }
        let mut rewritten = Zeroizing::new(plain.to_vec());
        let mut salt = [0; 32];
        SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| Error::SessionUnavailable)?;
        let aad = target.aad(scope)?;
        target
            .key(&salt, &aad)?
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key([0; 12]),
                aead::Aad::from(aad),
                &mut *rewritten,
            )
            .map_err(|_| Error::SessionUnavailable)?;
        Ok(SealedSession {
            version: 1,
            key_id: target.key_id.clone(),
            salt,
            ciphertext: rewritten.to_vec(),
        })
    }
    pub fn new(key_id: &str, key: Zeroizing<[u8; 32]>) -> Result<Self> {
        if !valid_key_id(key_id) {
            return Err(Error::Configuration);
        }
        Ok(Self {
            key_id: key_id.into(),
            master: key,
        })
    }
    fn aad(&self, scope: &CheckpointScope) -> Result<Vec<u8>> {
        if !scope.valid() {
            return Err(Error::SessionUnavailable);
        }
        serde_json::to_vec(&json!([
            "localai-cowork-v2:mcp-session",
            1,
            self.key_id,
            scope
        ]))
        .map_err(|_| Error::SessionUnavailable)
    }
    fn key(&self, salt: &[u8; 32], aad: &[u8]) -> Result<aead::LessSafeKey> {
        // A fresh 256-bit salt derives a new AEAD key for each checkpoint. The
        // fixed nonce is used once per derived key; no resettable nonce counter.
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, salt);
        let prk = salt.extract(self.master.as_ref());
        let info = [b"mcp-checkpoint-key-v1".as_slice(), aad];
        let okm = prk
            .expand(&info, &aead::CHACHA20_POLY1305)
            .map_err(|_| Error::SessionUnavailable)?;
        let mut bytes = Zeroizing::new([0u8; 32]);
        okm.fill(bytes.as_mut())
            .map_err(|_| Error::SessionUnavailable)?;
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, bytes.as_ref())
            .map(aead::LessSafeKey::new)
            .map_err(|_| Error::SessionUnavailable)
    }
    pub fn seal(&self, session: &Session, scope: &CheckpointScope) -> Result<SealedSession> {
        // A legacy SSE continuation depends on the original live GET stream. Never reinterpret it as a resumable Streamable-HTTP session.
        if !session.transport.supports_checkpoint() {
            return Err(Error::SessionUnavailable);
        }
        session.assert_ready()?;
        let catalog = session.catalog.as_ref().ok_or(Error::CatalogChanged)?;
        if catalog.sha256() != scope.catalog_sha256 || session.epoch == u64::MAX {
            return Err(Error::SessionUnavailable);
        }
        let aad = self.aad(scope)?;
        let plain = PlainCheckpoint {
            protocol: session.transport.protocol_version().into(),
            endpoint: session.transport.endpoint_identity().into(),
            identity: session.identity,
            epoch: session.epoch,
            catalog_sha256: catalog.sha256().into(),
            session_id: session
                .transport
                .checkpoint_session_id()
                .map(|b| Zeroizing::new(b.to_vec())),
            task_capabilities: session.task_capabilities.clone(),
            pending_task: session.pending_task.clone(),
            task_interactions: session.task_interactions,
        };
        let mut bytes =
            Zeroizing::new(serde_json::to_vec(&plain).map_err(|_| Error::SessionUnavailable)?);
        if bytes.len() > 8176 {
            return Err(Error::Limit);
        }
        let mut salt = [0u8; 32];
        SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| Error::SessionUnavailable)?;
        self.key(&salt, &aad)?
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key([0; 12]),
                aead::Aad::from(aad),
                &mut *bytes,
            )
            .map_err(|_| Error::SessionUnavailable)?;
        Ok(SealedSession {
            version: 1,
            key_id: self.key_id.clone(),
            salt,
            ciphertext: bytes.to_vec(),
        })
    }
    /// Rebuild the transport for the SAME remote session. Sends no initialize,
    /// discovery, tool call or other request. Credentials are supplied separately.
    pub fn open(
        &self,
        sealed: &SealedSession,
        scope: &CheckpointScope,
        endpoint: Endpoint,
        limits: Limits,
        definitions: Vec<Value>,
    ) -> Result<Session> {
        sealed.validate()?;
        if sealed.key_id != self.key_id {
            return Err(Error::SessionUnavailable);
        }
        let aad = self.aad(scope)?;
        let mut bytes = Zeroizing::new(sealed.ciphertext.clone());
        let plain = self
            .key(&sealed.salt, &aad)?
            .open_in_place(
                aead::Nonce::assume_unique_for_key([0; 12]),
                aead::Aad::from(aad),
                &mut bytes,
            )
            .map_err(|_| Error::SessionUnavailable)?;
        let checkpoint: PlainCheckpoint =
            serde_json::from_slice(plain).map_err(|_| Error::SessionUnavailable)?;
        let catalog = Catalog::from_definitions(definitions)?;
        if !matches!(
            checkpoint.protocol.as_str(),
            PROTOCOL_VERSION | "2025-03-26" | "2025-06-18"
        ) || checkpoint.endpoint != endpoint.url()
            || checkpoint.catalog_sha256 != scope.catalog_sha256
            || catalog.sha256() != scope.catalog_sha256
            || checkpoint.identity.is_nil()
            || checkpoint.epoch == u64::MAX
        {
            return Err(Error::SessionUnavailable);
        }
        let mut transport = Transport::new(endpoint, limits)?;
        transport.negotiate_protocol(&checkpoint.protocol)?;
        if checkpoint.protocol != PROTOCOL_VERSION
            && (checkpoint.pending_task.is_some()
                || checkpoint
                    .task_capabilities
                    .as_ref()
                    .is_some_and(|c| c.tools_call || c.cancel))
        {
            return Err(Error::SessionUnavailable);
        }
        if !transport.supports_checkpoint() {
            return Err(Error::SessionUnavailable);
        }
        transport.restore_session_id(checkpoint.session_id.as_ref().map(|v| v.as_slice()))?;
        Ok(Session {
            transport,
            identity: checkpoint.identity,
            epoch: checkpoint.epoch,
            ready: true,
            catalog: Some(catalog),
            task_capabilities: checkpoint.task_capabilities,
            pending_task: checkpoint.pending_task,
            task_interactions: checkpoint.task_interactions
                && checkpoint.protocol == PROTOCOL_VERSION,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Session, CheckpointScope) {
        let catalog = Catalog::from_definitions(vec![
            json!({"name":"inspect","inputSchema":{"type":"object"}}),
        ])
        .unwrap();
        let scope = CheckpointScope {
            project_id: "project".into(),
            run_id: "run".into(),
            server_id: "server".into(),
            connection_record_id: "connection".into(),
            server_revision: 1,
            checkpoint_revision: 2,
            configuration_sha256: "a".repeat(64),
            catalog_sha256: catalog.sha256().into(),
        };
        let mut transport = Transport::new(
            Endpoint::new("http://127.0.0.1:1/mcp", None).unwrap(),
            Limits::default(),
        )
        .unwrap();
        transport
            .restore_session_id(Some(b"private-session-header"))
            .unwrap();
        (
            Session {
                transport,
                identity: Uuid::new_v4(),
                epoch: 1,
                ready: true,
                catalog: Some(catalog),
                task_capabilities: None,
                pending_task: None,
                task_interactions: false,
            },
            scope,
        )
    }
    fn open(
        cipher: &SessionCipher,
        sealed: &SealedSession,
        scope: &CheckpointScope,
    ) -> Result<Session> {
        cipher.open(
            sealed,
            scope,
            Endpoint::new("http://127.0.0.1:1/mcp", None).unwrap(),
            Limits::default(),
            vec![json!({"name":"inspect","inputSchema":{"type":"object"}})],
        )
    }
    #[test]
    fn encrypted_checkpoint_restores_identity_epoch_and_secret_without_network() {
        let (session, scope) = fixture();
        let cipher = SessionCipher::new("test", Zeroizing::new([7; 32])).unwrap();
        let mut salts = std::collections::HashSet::new();
        for _ in 0..32 {
            let sealed = cipher.seal(&session, &scope).unwrap();
            assert!(salts.insert(sealed.salt));
            let encoded = serde_json::to_string(&sealed).unwrap();
            assert!(!encoded.contains("private-session-header"));
            let reopened = open(&cipher, &sealed, &scope).unwrap();
            assert_eq!(reopened.connection_id(), session.connection_id());
            assert_eq!(reopened.epoch(), 1);
            assert_eq!(
                reopened.transport.checkpoint_session_id(),
                Some(b"private-session-header".as_slice())
            );
        }
    }
    #[test]
    fn wrong_key_modified_envelope_and_every_scope_field_fail_authentication() {
        let (session, scope) = fixture();
        let cipher = SessionCipher::new("test", Zeroizing::new([7; 32])).unwrap();
        let sealed = cipher.seal(&session, &scope).unwrap();
        for key in [
            SessionCipher::new("test", Zeroizing::new([8; 32])).unwrap(),
            SessionCipher::new("other", Zeroizing::new([7; 32])).unwrap(),
        ] {
            assert!(open(&key, &sealed, &scope).is_err());
        }
        for field in [
            "project_id",
            "run_id",
            "server_id",
            "connection_record_id",
            "server_revision",
            "checkpoint_revision",
            "configuration_sha256",
            "catalog_sha256",
        ] {
            let mut value = serde_json::to_value(&scope).unwrap();
            value[field] = if field.ends_with("revision") {
                json!(99)
            } else if field.ends_with("sha256") {
                json!("b".repeat(64))
            } else {
                json!("changed")
            };
            let changed: CheckpointScope = serde_json::from_value(value).unwrap();
            assert!(open(&cipher, &sealed, &changed).is_err(), "{field}");
        }
        for field in ["ciphertext", "salt", "version"] {
            let mut damaged = sealed.clone();
            match field {
                "ciphertext" => damaged.ciphertext[0] ^= 1,
                "salt" => damaged.salt[0] ^= 1,
                _ => damaged.version = 2,
            }
            assert!(open(&cipher, &damaged, &scope).is_err());
        }
    }
    #[test]
    fn unusable_sessions_and_different_endpoint_or_catalog_cannot_be_restored() {
        let (mut session, scope) = fixture();
        let cipher = SessionCipher::new("test", Zeroizing::new([7; 32])).unwrap();
        let sealed = cipher.seal(&session, &scope).unwrap();
        assert!(
            cipher
                .open(
                    &sealed,
                    &scope,
                    Endpoint::new("http://127.0.0.1:2/mcp", None).unwrap(),
                    Limits::default(),
                    vec![json!({"name":"inspect","inputSchema":{"type":"object"}})]
                )
                .is_err()
        );
        assert!(
            cipher
                .open(
                    &sealed,
                    &scope,
                    Endpoint::new("http://127.0.0.1:1/mcp", None).unwrap(),
                    Limits::default(),
                    vec![]
                )
                .is_err()
        );
        session.epoch = u64::MAX;
        assert!(cipher.seal(&session, &scope).is_err());
        session.epoch = 1;
        session.ready = false;
        assert!(cipher.seal(&session, &scope).is_err());
        session.ready = true;
        session.catalog = None;
        assert!(cipher.seal(&session, &scope).is_err());
    }
}
