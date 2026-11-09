//! Only the SDK's checked signed claims create this private subject evidence.
//! Session/flow/profile admission remains in the same Identity transaction.
//! Email is descriptive data and is never an existing-account lookup key.
use super::*;
use openidconnect::{
    AccessTokenHash, ClientId, Nonce, OAuth2TokenResponse, TokenResponse,
    core::{CoreClient, CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreTokenResponse},
};

pub(super) struct VerificationContext<'a> {
    pub metadata: &'a CoreProviderMetadata,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    pub groups_claim: &'a str,
    pub flow_started_at: i64,
    pub current: i64,
}

/// Neither serializable nor constructible from an HTTP/renderer DTO. The
/// containing store module owns the constructor and all authority consumers.
pub(super) struct VerifiedOidcSubject {
    pub issuer: String,
    pub subject: String,
    pub claims_sha256: String,
    pub expires_at: i64,
    pub authentication_time: Option<i64>,
    pub groups: Vec<String>,
}

pub(super) fn verify(
    context: VerificationContext<'_>,
    response: &CoreTokenResponse,
) -> Result<VerifiedOidcSubject> {
    if context.metadata.issuer().as_str() != context.issuer
        || context.client_id.is_empty()
        || context.client_id.len() > 512
        || context.nonce.len() < 32
        || context.nonce.len() > 256
        || context
            .current
            .checked_sub(context.flow_started_at)
            .is_none_or(|elapsed| !(0..=300_000).contains(&elapsed))
    {
        return Err(IdentityError::Unauthenticated);
    }
    let client = CoreClient::from_provider_metadata(
        context.metadata.clone(),
        ClientId::new(context.client_id.to_owned()),
        None,
    );
    // Symmetric client secrets and unsigned JWTs cannot substitute provider
    // signing keys. These algorithms are implemented by the pinned SDK.
    let verifier = client.id_token_verifier().set_allowed_algs([
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
        CoreJwsSigningAlgorithm::RsaSsaPssSha256,
        CoreJwsSigningAlgorithm::RsaSsaPssSha384,
        CoreJwsSigningAlgorithm::RsaSsaPssSha512,
        CoreJwsSigningAlgorithm::EcdsaP256Sha256,
        CoreJwsSigningAlgorithm::EcdsaP384Sha384,
        CoreJwsSigningAlgorithm::EdDsa,
    ]);
    let token = response.id_token().ok_or(IdentityError::Unauthenticated)?;
    let claims = token
        .claims(&verifier, &Nonce::new(context.nonce.to_owned()))
        .map_err(|_| IdentityError::Unauthenticated)?;
    if claims.issuer().as_str() != context.issuer
        || !claims
            .audiences()
            .iter()
            .any(|audience| audience.as_str() == context.client_id)
        || claims.audiences().len() > 1 && claims.authorized_party().is_none()
        || claims
            .authorized_party()
            .is_some_and(|party| party.as_str() != context.client_id)
        || claims.expiration().timestamp_millis() <= context.current
        || claims.issue_time().timestamp_millis() > context.current.saturating_add(30_000)
        || claims.issue_time().timestamp_millis() < context.flow_started_at.saturating_sub(300_000)
        || claims
            .auth_time()
            .is_some_and(|time| time.timestamp_millis() > context.current.saturating_add(30_000))
    {
        return Err(IdentityError::Unauthenticated);
    }
    if let Some(expected) = claims.access_token_hash() {
        let actual = AccessTokenHash::from_token(
            response.access_token(),
            token
                .signing_alg()
                .map_err(|_| IdentityError::Unauthenticated)?,
            token
                .signing_key(&verifier)
                .map_err(|_| IdentityError::Unauthenticated)?,
        )
        .map_err(|_| IdentityError::Unauthenticated)?;
        if actual != *expected {
            return Err(IdentityError::Unauthenticated);
        }
    }
    let subject = claims.subject().as_str();
    if subject.is_empty() || subject.len() > 512 || subject.chars().any(char::is_control) {
        return Err(IdentityError::Unauthenticated);
    }
    let email = claims.email().map(|value| value.as_str().to_owned());
    if email
        .as_ref()
        .is_some_and(|value| value.len() > 200 || value.chars().any(char::is_control))
    {
        return Err(IdentityError::Unauthenticated);
    }
    // ToString retains the exact SDK-verified JWT signing input. Extract only
    // after the same token's JWS/issuer/audience/nonce/time/hash checks above.
    use base64::Engine;
    let signed = token.to_string();
    let encoded = signed
        .split('.')
        .nth(1)
        .ok_or(IdentityError::Unauthenticated)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| IdentityError::Unauthenticated)?;
    let signed_claims: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| IdentityError::Unauthenticated)?;
    let mut groups = Vec::new();
    if let Some(value) = signed_claims.get(context.groups_claim) {
        let values = value.as_array().ok_or(IdentityError::Unauthenticated)?;
        if values.len() > 1000 {
            return Err(IdentityError::Unauthenticated);
        }
        for value in values {
            let value = value.as_str().ok_or(IdentityError::Unauthenticated)?;
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(IdentityError::Unauthenticated);
            }
            if !groups.iter().any(|v| v == value) {
                groups.push(value.to_owned());
            }
        }
    }
    groups.sort();
    if bytes.len() > 512 * 1024 {
        return Err(IdentityError::Unauthenticated);
    }
    Ok(VerifiedOidcSubject {
        issuer: context.issuer.into(),
        subject: subject.into(),
        claims_sha256: format!("{:x}", Sha256::digest(bytes)),
        expires_at: claims.expiration().timestamp_millis(),
        authentication_time: claims.auth_time().map(|time| time.timestamp_millis()),
        groups,
    })
}
