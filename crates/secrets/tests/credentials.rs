use cowork_secrets::*;
use serde_json::json;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys {
    id: KeyId,
    bytes: u8,
}
impl KeyStore for Keys {
    fn load(&self, id: &KeyId) -> Result<Zeroizing<[u8; 32]>> {
        if id != &self.id {
            return Err(Error::Missing);
        }
        Ok(Zeroizing::new([self.bytes; 32]))
    }
}
fn fixture() -> (Keys, CredentialScope) {
    (
        Keys {
            id: KeyId {
                instance: Uuid::new_v4(),
                key: Uuid::new_v4(),
            },
            bytes: 7,
        },
        CredentialScope {
            project_id: "project".into(),
            server_id: "server".into(),
            reference: "credential".into(),
            revision: 1,
            endpoint: "https://example.test/mcp".into(),
        },
    )
}
#[test]
fn bound_ciphertexts_roundtrip_short_and_maximum_bearers_without_plaintext_serialization() {
    let (keys, scope) = fixture();
    for bearer in ["synthetic-only".to_string(), "a".repeat(4096)] {
        let sealed = seal_bearer(&keys, keys.id, &scope, &bearer).unwrap();
        assert_eq!(*open_bearer(&keys, &scope, &sealed).unwrap(), bearer);
        let encoded = serde_json::to_string(&sealed).unwrap();
        assert!(!encoded.contains(&bearer));
        let second = seal_bearer(&keys, keys.id, &scope, &bearer).unwrap();
        assert!(sealed != second);
    }
    assert_eq!(
        KeyId::from_cipher_id(&keys.id.cipher_id().unwrap()).unwrap(),
        keys.id
    );
}
#[test]
fn every_scope_field_key_and_ciphertext_is_authenticated() {
    let (keys, scope) = fixture();
    let sealed = seal_bearer(&keys, keys.id, &scope, "synthetic-only").unwrap();
    for field in [
        "project_id",
        "server_id",
        "reference",
        "revision",
        "endpoint",
    ] {
        let mut changed = serde_json::to_value(&scope).unwrap();
        changed[field] = if field == "revision" {
            json!(2)
        } else {
            json!("other")
        };
        let changed: CredentialScope = serde_json::from_value(changed).unwrap();
        assert!(open_bearer(&keys, &changed, &sealed).is_err(), "{field}");
    }
    assert!(
        open_bearer(
            &Keys {
                id: keys.id,
                bytes: 8
            },
            &scope,
            &sealed
        )
        .is_err()
    );
    for field in ["salt", "ciphertext", "version", "key_id"] {
        let mut changed = serde_json::to_value(&sealed).unwrap();
        match field {
            "version" => changed[field] = json!(2),
            "key_id" => changed[field]["key"] = json!(Uuid::new_v4()),
            _ => changed[field][0] = json!((changed[field][0].as_u64().unwrap() + 1) % 256),
        }
        let changed = serde_json::from_value(changed).unwrap();
        assert!(open_bearer(&keys, &scope, &changed).is_err(), "{field}");
    }
}
#[test]
fn invalid_secrets_scopes_and_locators_fail_without_plaintext_errors() {
    let (keys, mut scope) = fixture();
    for bearer in ["", "a b", "bad\r\nvalue", "ümlaut", &"a".repeat(4097)] {
        assert!(seal_bearer(&keys, keys.id, &scope, bearer).is_err());
    }
    scope.revision = 0;
    assert!(seal_bearer(&keys, keys.id, &scope, "synthetic-only").is_err());
    for bad in [
        "",
        "fixture",
        "native.00000000000000000000000000000000.00000000000000000000000000000000",
    ] {
        assert!(KeyId::from_cipher_id(bad).is_err());
    }
    assert!(NativeKeyStore::open(Uuid::nil()).is_err());
}
#[test]
fn native_confirmation_detects_changed_key_and_locator_without_recreating_it() {
    let (keys, _) = fixture();
    let proof = prove_key(&keys, &keys.id).unwrap();
    verify_key(&keys, &proof).unwrap();
    assert!(
        verify_key(
            &Keys {
                id: keys.id,
                bytes: 8
            },
            &proof
        )
        .is_err()
    );
    let mut wrong = proof.clone();
    wrong.key.key = Uuid::new_v4();
    assert!(verify_key(&keys, &wrong).is_err());
    let encoded = serde_json::to_string(&proof).unwrap();
    let decoded: KeyProof = serde_json::from_str(&encoded).unwrap();
    verify_key(&keys, &decoded).unwrap();
}
