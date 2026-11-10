use cowork_contracts::*;
use cowork_domain::{Error, canonical_hash};
use cowork_runtime::external_memory::retrieve;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{Store, library_config::LibraryOwner};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;
use zeroize::Zeroizing;
struct Keys;
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        Ok(Zeroizing::new([19; 32]))
    }
}
#[tokio::test]
async fn server_actor_cannot_retrieve_daemon_loopback_or_private_https_memory() {
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let app = axum::Router::new().route(
        "/memory",
        axum::routing::any(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!([]))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let loopback = format!("http://{}/memory", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let identity = s.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, issued) = identity
        .bootstrap(
            &bootstrap,
            "external-memory-actor",
            "Private actor",
            "SyntheticPassword-2026!",
            "Fixture",
        )
        .await
        .unwrap();
    let actor = identity.authenticate_session(&issued.token).await.unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    for endpoint in [
        loopback,
        "https://127.0.0.1/memory".into(),
        "https://[::1]/memory".into(),
        "https://169.254.169.254/latest/meta-data/".into(),
        "https://10.0.0.1/memory".into(),
        "https://memory.example.test:8443/memory".into(),
    ] {
        for method in ["GET", "POST"] {
            let id = Uuid::new_v4().to_string();
            let provider = WriteExternalMemoryProvider {
                request_id: Uuid::new_v4(),
                expected_revision: 0,
                value: ExternalMemoryDefinition {
                    name: "Actor private target".into(),
                    endpoint: endpoint.clone(),
                    method: method.into(),
                    request_body: if method == "POST" {
                        Some(json!({"query":"private"}))
                    } else {
                        None
                    },
                    items_pointer: "".into(),
                    content_field: "text".into(),
                    id_field: None,
                    auth_scheme: "none".into(),
                },
                api_key: None,
                remove_key: false,
            };
            s.write_external_memory_provider(
                LibraryOwner::Actor(&actor),
                &id,
                &provider,
                &Keys,
                key,
                false,
            )
            .await
            .unwrap();
            let request = RetrieveExternalMemory {
                request_id: Uuid::new_v4(),
                expected_revision: 1,
            };
            let error = retrieve(&s, LibraryOwner::Actor(&actor), &id, &request, &Keys)
                .await
                .unwrap_err();
            assert!(matches!(error, Error::Validation(_)), "{endpoint}: {error}");
            assert!(matches!(
                retrieve(&s, LibraryOwner::Actor(&actor), &id, &request, &Keys).await,
                Err(Error::UncertainEffect)
            ));
        }
    }
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    server.abort();
}
#[tokio::test]
async fn external_http_memory_is_captured_once_and_imported_as_immutable_user_evidence() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let app = axum::Router::new().route(
        "/memory",
        axum::routing::post(
            move |headers: axum::http::HeaderMap,
                  axum::Json(body): axum::Json<serde_json::Value>| {
                let count = seen.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer fixture-private-key");
                    assert_eq!(body, json!({"query":"language"}));
                    count.fetch_add(1, Ordering::SeqCst);
                    axum::Json(
                        json!({"results":[{"id":"language","text":"User prefers German replies"}]}),
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/memory", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite");
    let s = Store::sqlite(&path).await.unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let id = Uuid::new_v4().to_string();
    let write = WriteExternalMemoryProvider {
        request_id: Uuid::new_v4(),
        expected_revision: 0,
        value: ExternalMemoryDefinition {
            name: "Private memory".into(),
            endpoint: endpoint.clone(),
            method: "POST".into(),
            request_body: Some(json!({"query":"language"})),
            items_pointer: "/results".into(),
            content_field: "text".into(),
            id_field: Some("id".into()),
            auth_scheme: "Bearer".into(),
        },
        api_key: Some("fixture-private-key".into()),
        remove_key: false,
    };
    let provider = s
        .write_external_memory_provider(LibraryOwner::Local, &id, &write, &Keys, key, false)
        .await
        .unwrap();
    assert_eq!(provider.revision, 1);
    assert!(
        !serde_json::to_string(&provider)
            .unwrap()
            .contains("fixture-private-key")
    );
    assert_eq!(
        canonical_hash(&provider),
        canonical_hash(
            &s.write_external_memory_provider(LibraryOwner::Local, &id, &write, &Keys, key, false)
                .await
                .unwrap()
        )
    );
    let q = RetrieveExternalMemory {
        request_id: Uuid::new_v4(),
        expected_revision: 1,
    };
    let receipt = retrieve(&s, LibraryOwner::Local, &id, &q, &Keys)
        .await
        .unwrap();
    assert_eq!(receipt.items[0].content, "User prefers German replies");
    assert_eq!(receipt.response_sha256.len(), 64);
    s.close().await;
    let s = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        canonical_hash(&receipt),
        canonical_hash(
            &retrieve(&s, LibraryOwner::Local, &id, &q, &Keys)
                .await
                .unwrap()
        )
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let project = s
        .create_project(CreateProject {
            name: "Reviewed imports".into(),
        })
        .await
        .unwrap();
    let imported = s
        .import_knowledge(
            &project.id,
            KnowledgeImportRequest {
                idempotency_key: Uuid::new_v4(),
                documents: vec![WriteKnowledge {
                    location: None,
                    id: None,
                    expected_revision: 0,
                    kind: KnowledgeKind::Memory,
                    title: "External memory language".into(),
                    content: receipt.items[0].content.clone(),
                    tags: vec!["external-memory".into()],
                    memory: Some(MemoryMetadata {
                        scope: MemoryScope::User,
                        ..Default::default()
                    }),
                    provenance: Some(KnowledgeProvenance {
                        format: 1,
                        origin: KnowledgeOrigin::Retrieved,
                        sources: vec![KnowledgeSource {
                            format: 1,
                            source_uri: Some(format!(
                                "urn:cowork:external-memory/{}",
                                receipt.request_id
                            )),
                            file_id: None,
                            artifact_id: None,
                            revision: "1".into(),
                            sha256: receipt.response_sha256.clone(),
                            locator: "language".into(),
                            verification: KnowledgeSourceVerification::Declared,
                        }],
                    }),
                }],
            },
        )
        .await
        .unwrap();
    assert_eq!(imported.documents[0].content, receipt.items[0].content);
    assert_eq!(
        imported.documents[0].provenance.sources[0].verification,
        KnowledgeSourceVerification::SnapshotVerified
    );
    let deletion = DeleteExternalMemoryProvider {
        request_id: Uuid::new_v4(),
        expected_revision: 1,
    };
    let deleted = s
        .delete_external_memory_provider(LibraryOwner::Local, &id, &deletion)
        .await
        .unwrap();
    assert_eq!(
        deleted,
        s.delete_external_memory_provider(LibraryOwner::Local, &id, &deletion)
            .await
            .unwrap()
    );
    assert!(
        s.external_memory_providers(LibraryOwner::Local)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        retrieve(
            &s,
            LibraryOwner::Local,
            &id,
            &RetrieveExternalMemory {
                request_id: Uuid::new_v4(),
                expected_revision: 1
            },
            &Keys
        )
        .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        canonical_hash(&receipt),
        canonical_hash(
            &retrieve(&s, LibraryOwner::Local, &id, &q, &Keys)
                .await
                .unwrap()
        )
    );
    server.abort();
}
#[tokio::test]
async fn failed_external_dispatch_is_not_retried_and_secret_urls_are_rejected() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let key = KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let id = Uuid::new_v4().to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/memory", listener.local_addr().unwrap());
    drop(listener);
    let mut q = WriteExternalMemoryProvider {
        request_id: Uuid::new_v4(),
        expected_revision: 0,
        value: ExternalMemoryDefinition {
            name: "Unavailable".into(),
            endpoint: endpoint.clone(),
            method: "GET".into(),
            request_body: None,
            items_pointer: "".into(),
            content_field: "text".into(),
            id_field: None,
            auth_scheme: "none".into(),
        },
        api_key: None,
        remove_key: false,
    };
    q.value.endpoint.push_str("?token=secret");
    assert!(
        s.write_external_memory_provider(LibraryOwner::Local, &id, &q, &Keys, key, false)
            .await
            .is_err()
    );
    q.value.endpoint = endpoint;
    s.write_external_memory_provider(LibraryOwner::Local, &id, &q, &Keys, key, false)
        .await
        .unwrap();
    let q = RetrieveExternalMemory {
        request_id: Uuid::new_v4(),
        expected_revision: 1,
    };
    assert!(
        retrieve(&s, LibraryOwner::Local, &id, &q, &Keys)
            .await
            .is_err()
    );
    assert!(matches!(
        retrieve(&s, LibraryOwner::Local, &id, &q, &Keys).await,
        Err(Error::UncertainEffect)
    ));
}
