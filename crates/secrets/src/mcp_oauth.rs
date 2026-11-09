//! OAuth/task ciphertext belongs to its original owner and operation. It is not
//! a bearer credential, a refresh permit or permission to replay remote IO.
use super::*;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpSecretScope {
    pub owner_kind: String,
    pub owner_id: String,
    pub project_id: String,
    pub server_id: String,
    pub record_id: String,
    pub revision: i64,
    pub epoch: Uuid,
    pub configuration_sha256: String,
    pub purpose: String,
}
impl McpSecretScope {
    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.owner_kind.as_str(),
            "native" | "personal" | "team" | "offline"
        ) || self.owner_kind == "offline" && !self.purpose.starts_with("task_")
            || self.revision <= 0
            || self.epoch.is_nil()
            || [
                &self.owner_id,
                &self.project_id,
                &self.server_id,
                &self.record_id,
            ]
            .iter()
            .any(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
            || self.configuration_sha256.len() != 64
            || !self
                .configuration_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(
                self.purpose.as_str(),
                "oauth_profile"
                    | "oauth_flow"
                    | "oauth_callback"
                    | "oauth_token"
                    | "oauth_response"
                    | "task_ack"
                    | "task_checkpoint"
                    | "task_response"
                    | "task_interaction_request"
                    | "task_interaction_decision"
                    | "task_interaction_model_intent"
                    | "task_interaction_model_response"
                    | "task_interaction_response"
            )
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SealedMcpBlob {
    version: u32,
    key_id: KeyId,
    salt: [u8; 32],
    ciphertext: Vec<u8>,
}
impl SealedMcpBlob {
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }
    pub fn validate(&self) -> Result<()> {
        self.key_id.validate()?;
        if self.version != 1 || !(17..=4_194_320).contains(&self.ciphertext.len()) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
fn binding(scope: &McpSecretScope, key: KeyId) -> Result<Vec<u8>> {
    scope.validate()?;
    key.validate()?;
    if scope.owner_kind == "offline" && scope.owner_id != key.instance.to_string() {
        return Err(Error::Invalid);
    }
    serde_json::to_vec(&("cowork:mcp:original-secret:v1", key, scope)).map_err(|_| Error::Invalid)
}
pub fn seal_mcp_blob(
    keys: &dyn KeyStore,
    key: KeyId,
    scope: &McpSecretScope,
    plain: &[u8],
) -> Result<SealedMcpBlob> {
    scope.validate()?;
    let max = if scope.purpose.starts_with("oauth_") {
        131_072
    } else {
        4_194_304
    };
    if plain.is_empty() || plain.len() > max {
        return Err(Error::Invalid);
    }
    let mut salt = [0; 32];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| Error::Unavailable)?;
    let aad = binding(scope, key)?;
    let master = keys.load(&key)?;
    let mut ciphertext = Zeroizing::new(plain.to_vec());
    derived(master.as_ref(), &salt, &aad)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key([0; 12]),
            aead::Aad::from(aad),
            &mut *ciphertext,
        )
        .map_err(|_| Error::Invalid)?;
    Ok(SealedMcpBlob {
        version: 1,
        key_id: key,
        salt,
        ciphertext: ciphertext.to_vec(),
    })
}
pub fn open_mcp_blob(
    keys: &dyn KeyStore,
    scope: &McpSecretScope,
    sealed: &SealedMcpBlob,
) -> Result<Zeroizing<Vec<u8>>> {
    sealed.validate()?;
    let aad = binding(scope, sealed.key_id)?;
    let master = keys.load(&sealed.key_id)?;
    let mut bytes = Zeroizing::new(sealed.ciphertext.clone());
    let plain = derived(master.as_ref(), &sealed.salt, &aad)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key([0; 12]),
            aead::Aad::from(aad),
            &mut bytes,
        )
        .map_err(|_| Error::Invalid)?;
    let result = Zeroizing::new(plain.to_vec());
    let max = if scope.purpose.starts_with("oauth_") {
        131_072
    } else {
        4_194_304
    };
    if result.is_empty() || result.len() > max {
        return Err(Error::Invalid);
    }
    Ok(result)
}
pub fn rewrap_mcp_blob(
    keys: &dyn KeyStore,
    scope: &McpSecretScope,
    sealed: &SealedMcpBlob,
    target: KeyId,
) -> Result<SealedMcpBlob> {
    if sealed.key_id == target {
        let _authenticated = open_mcp_blob(keys, scope, sealed)?;
        return Ok(sealed.clone());
    }
    if sealed.key_id.instance != target.instance {
        return Err(Error::Invalid);
    }
    let plain = open_mcp_blob(keys, scope, sealed)?;
    seal_mcp_blob(keys, target, scope, &plain)
}
