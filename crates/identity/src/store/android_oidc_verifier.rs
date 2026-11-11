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

#[cfg(test)]
mod tests {
    use super::{
        CoreProviderMetadata, CoreTokenResponse, VerificationContext, VerifiedOidcSubject, verify,
    };
    use crate::{IdentityError, Result};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use openidconnect::core::CoreJsonWebKeySet;
    use ring::{
        rand::SystemRandom,
        signature::{self, KeyPair},
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    const ISSUER: &str = "https://offline-issuer.invalid/";
    const CLIENT: &str = "offline-android-client";
    const NONCE: &str = "offline-nonce-with-at-least-thirty-two-bytes";

    enum SigningKey {
        Ec(signature::EcdsaKeyPair),
        Ed25519(signature::Ed25519KeyPair),
    }

    impl SigningKey {
        fn public_key(&self) -> &[u8] {
            match self {
                Self::Ec(key) => key.public_key().as_ref(),
                Self::Ed25519(key) => key.public_key().as_ref(),
            }
        }

        fn sign(&self, input: &[u8]) -> Vec<u8> {
            match self {
                Self::Ec(key) => key
                    .sign(&SystemRandom::new(), input)
                    .unwrap()
                    .as_ref()
                    .to_vec(),
                Self::Ed25519(key) => key.sign(input).as_ref().to_vec(),
            }
        }
    }

    struct Fixture {
        algorithm: &'static str,
        key: SigningKey,
        metadata: CoreProviderMetadata,
        now: i64,
    }

    impl Fixture {
        fn new(algorithm: &'static str) -> Self {
            // Independent Ring signing exercises the SDK's verification boundary.
            // Generated private material stays in RAM; no HTTP, DB, or key files.
            let rng = SystemRandom::new();
            let key = if algorithm == "EdDSA" {
                let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
                SigningKey::Ed25519(signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap())
            } else {
                let signing = match algorithm {
                    "ES256" => &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                    "ES384" => &signature::ECDSA_P384_SHA384_FIXED_SIGNING,
                    _ => panic!("unsupported offline fixture algorithm"),
                };
                let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(signing, &rng).unwrap();
                SigningKey::Ec(
                    signature::EcdsaKeyPair::from_pkcs8(signing, pkcs8.as_ref(), &rng).unwrap(),
                )
            };
            let public = key.public_key();
            let jwk = if algorithm == "EdDSA" {
                json!({"kty":"OKP", "crv":"Ed25519", "x":URL_SAFE_NO_PAD.encode(public),
                    "kid":"offline-key", "use":"sig", "alg":algorithm})
            } else {
                let (width, curve) = if algorithm == "ES256" {
                    (32, "P-256")
                } else {
                    (48, "P-384")
                };
                assert_eq!(public.len(), 1 + 2 * width);
                assert_eq!(public[0], 4);
                json!({"kty":"EC", "crv":curve,
                    "x":URL_SAFE_NO_PAD.encode(&public[1..1 + width]),
                    "y":URL_SAFE_NO_PAD.encode(&public[1 + width..]),
                    "kid":"offline-key", "use":"sig", "alg":algorithm})
            };
            let jwks: CoreJsonWebKeySet = serde_json::from_value(json!({"keys":[jwk]})).unwrap();
            let metadata: CoreProviderMetadata = serde_json::from_value(json!({
                "issuer":ISSUER,
                "authorization_endpoint":"https://offline-issuer.invalid/authorize",
                "token_endpoint":"https://offline-issuer.invalid/token",
                "jwks_uri":"https://offline-issuer.invalid/keys",
                "response_types_supported":["code"],
                "subject_types_supported":["public"],
                "id_token_signing_alg_values_supported":[algorithm]
            }))
            .unwrap();
            Self {
                algorithm,
                key,
                metadata: metadata.set_jwks(jwks),
                now: chrono::Utc::now().timestamp() * 1000,
            }
        }

        fn claims(&self) -> Value {
            json!({"iss":ISSUER, "aud":CLIENT, "sub":"verified-offline-subject", "nonce":NONCE,
                "iat":self.now / 1000, "exp":self.now / 1000 + 300,
                "auth_time":self.now / 1000 - 1, "groups":["zeta", "alpha", "zeta"]})
        }

        fn input(&self, algorithm: &str, claims: &Value) -> String {
            format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&json!({"alg":algorithm, "kid":"offline-key", "typ":"JWT"}))
                        .unwrap()
                ),
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
            )
        }

        fn token(&self, claims: &Value) -> String {
            let input = self.input(self.algorithm, claims);
            format!(
                "{input}.{}",
                URL_SAFE_NO_PAD.encode(self.key.sign(input.as_bytes()))
            )
        }

        fn verify(&self, token: &str) -> Result<VerifiedOidcSubject> {
            let response: CoreTokenResponse = serde_json::from_value(json!({
                "access_token":"offline-access-token", "token_type":"bearer", "id_token":token
            }))
            .unwrap();
            verify(
                VerificationContext {
                    metadata: &self.metadata,
                    issuer: ISSUER,
                    client_id: CLIENT,
                    nonce: NONCE,
                    groups_claim: "groups",
                    flow_started_at: self.now - 1000,
                    current: self.now,
                },
                &response,
            )
        }

        fn reject(&self, token: &str) {
            assert!(matches!(
                self.verify(token),
                Err(IdentityError::Unauthenticated)
            ));
        }
    }

    #[test]
    fn independently_signed_ec_and_eddsa_tokens_create_checked_subject_and_groups() {
        for algorithm in ["ES256", "ES384", "EdDSA"] {
            let fixture = Fixture::new(algorithm);
            let claims = fixture.claims();
            let subject = fixture.verify(&fixture.token(&claims)).unwrap();
            assert_eq!(subject.issuer, ISSUER);
            assert_eq!(subject.subject, "verified-offline-subject");
            assert_eq!(subject.groups, vec!["alpha".to_owned(), "zeta".to_owned()]);
            assert_eq!(subject.expires_at, fixture.now + 300_000);
            assert_eq!(subject.authentication_time, Some(fixture.now - 1000));
            assert_eq!(
                subject.claims_sha256,
                format!("{:x}", Sha256::digest(serde_json::to_vec(&claims).unwrap()))
            );
        }
    }

    #[test]
    fn correctly_signed_wrong_issuer_audience_or_nonce_cannot_create_subject() {
        let fixture = Fixture::new("ES256");
        assert!(fixture.verify(&fixture.token(&fixture.claims())).is_ok());
        for (claim, value) in [
            ("iss", json!("https://another-issuer.invalid/")),
            ("aud", json!("another-client")),
            (
                "nonce",
                json!("different-nonce-with-at-least-thirty-two-bytes"),
            ),
        ] {
            let mut claims = fixture.claims();
            claims[claim] = value;
            fixture.reject(&fixture.token(&claims));
        }
    }

    #[test]
    fn authorized_party_and_multiple_audiences_keep_product_policy() {
        let fixture = Fixture::new("ES256");
        let mut claims = fixture.claims();
        claims["azp"] = json!(CLIENT);
        assert!(fixture.verify(&fixture.token(&claims)).is_ok());
        claims["azp"] = json!("another-client");
        fixture.reject(&fixture.token(&claims));

        // Duplicate client audiences avoid an unrelated untrusted-audience
        // rejection and exercise our own requirement for an explicit azp.
        claims["aud"] = json!([CLIENT, CLIENT]);
        claims["azp"] = json!(CLIENT);
        assert!(fixture.verify(&fixture.token(&claims)).is_ok());
        claims.as_object_mut().unwrap().remove("azp");
        fixture.reject(&fixture.token(&claims));

        claims["aud"] = json!([CLIENT, "untrusted-client"]);
        claims["azp"] = json!(CLIENT);
        fixture.reject(&fixture.token(&claims));
    }

    #[test]
    fn tampered_provider_signature_cannot_create_subject() {
        let fixture = Fixture::new("ES256");
        let token = fixture.token(&fixture.claims());
        assert!(fixture.verify(&token).is_ok());
        let (input, encoded_signature) = token.rsplit_once('.').unwrap();
        let mut signature = URL_SAFE_NO_PAD.decode(encoded_signature).unwrap();
        signature[0] ^= 1;
        fixture.reject(&format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)));
    }

    #[test]
    fn unsigned_and_symmetric_algorithm_substitution_cannot_create_subject() {
        let fixture = Fixture::new("ES256");
        let claims = fixture.claims();
        assert!(fixture.verify(&fixture.token(&claims)).is_ok());
        fixture.reject(&format!("{}.", fixture.input("none", &claims)));

        // A public provider key must not become an HMAC client secret.
        let input = fixture.input("HS256", &claims);
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, fixture.key.public_key());
        let signature = ring::hmac::sign(&key, input.as_bytes());
        fixture.reject(&format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ));
    }
}
