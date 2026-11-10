//! Local SQLite authority/materialization smoke. No sockets, providers, tools, real accounts or keys.
use cowork_domain::Error;
use cowork_storage::{AndroidWireSyncChange, Store};
use serde_json::{Value, json};
use uuid::Uuid;
struct SyntheticKey(cowork_secrets::KeyId);
impl cowork_secrets::KeyStore for SyntheticKey {
    fn load(
        &self,
        id: &cowork_secrets::KeyId,
    ) -> cowork_secrets::Result<zeroize::Zeroizing<[u8; 32]>> {
        if *id != self.0 {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(zeroize::Zeroizing::new([42; 32]))
    }
}
fn change(
    device: Uuid,
    kind: &str,
    id: Uuid,
    base: i64,
    payload: Option<Value>,
) -> AndroidWireSyncChange {
    AndroidWireSyncChange {
        schema_version: 2,
        operation_id: Uuid::new_v4(),
        device_id: device,
        entity_type: kind.into(),
        entity_id: id,
        base_revision: base,
        operation: if payload.is_some() {
            "upsert"
        } else {
            "delete"
        }
        .into(),
        payload,
        client_timestamp: chrono::Utc::now().to_rfc3339(),
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let store = Store::sqlite(&scratch.path().join("wire.db")).await?;
    let identity = store.identity();
    let bootstrap = "synthetic-bootstrap-only-for-wire-sync-smoke";
    identity
        .initialize_credentials(bootstrap, "synthetic-settlement-only-for-wire-sync-smoke")
        .await?;
    let (_, session) = identity
        .bootstrap(
            bootstrap,
            "wire-owner",
            "Wire owner",
            "Synthetic-wire-password-only!",
            "Wire tenant",
        )
        .await?;
    let actor = identity.authenticate_session(&session.token).await?;
    let device = Uuid::new_v4();
    store.android_wire_bind_device(&actor, device).await?;
    let project = Uuid::new_v4();
    let thread = Uuid::new_v4();
    let task = Uuid::new_v4();
    let message = Uuid::new_v4();
    let children = vec![
        change(
            device,
            "message",
            message,
            0,
            Some(
                json!({"thread_id":thread,"role":"assistant","content":"Imported inert history","timestamp":1234567890000i64,"durable_run_id":Uuid::new_v4()}),
            ),
        ),
        change(
            device,
            "task",
            task,
            0,
            Some(
                json!({"thread_id":thread,"title":"Local task","description":"Actual frozen instructions","runner":"model"}),
            ),
        ),
        change(
            device,
            "thread",
            thread,
            0,
            Some(json!({"title":"Local thread"})),
        ),
    ];
    let original = store
        .android_wire_sync_push(&actor, children.clone())
        .await?;
    assert!(
        store
            .android_personal_thread(&actor, thread)
            .await?
            .is_none()
    );
    store.android_wire_sync_push(&actor,vec![change(device,"project",project,0,Some(json!({"title":"Actual project","instructions":"Actual description","project_kind":"private","thread_ids":[thread]})))]).await?;
    let stored = store
        .android_personal_project(&actor, project)
        .await?
        .unwrap();
    assert_eq!(stored["owner_user_id"], json!(actor.identity_id()));
    assert!(stored["team_id"].is_null());
    assert_eq!(
        store
            .android_personal_thread(&actor, thread)
            .await?
            .unwrap()["project_id"],
        json!(project)
    );
    let tasks = store.android_tasks(&actor, project).await?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["instructions"], "Actual frozen instructions");
    let messages = store.android_personal_messages(&actor, thread).await?;
    assert_eq!(messages.len(), 1);
    assert!(messages[0]["run_id"].is_null());
    assert_eq!(messages[0]["content"]["content"], "Imported inert history");
    assert_eq!(
        store
            .android_wire_sync_push(&actor, children.clone())
            .await?,
        original
    );
    let mut collision = children[0].clone();
    collision.payload = Some(
        json!({"thread_id":thread,"role":"assistant","content":"Changed bytes under old operation ID"}),
    );
    assert!(matches!(
        store.android_wire_sync_push(&actor, vec![collision]).await,
        Err(Error::Conflict)
    ));
    identity
        .register_local_identity(
            "wire-other",
            "Other owner",
            "Synthetic-other-password-only!",
        )
        .await?;
    let other = identity
        .login("wire-other", "Synthetic-other-password-only!", 3_600_000)
        .await?;
    let other = identity.authenticate_session(&other.token).await?;
    let otherdevice = Uuid::new_v4();
    store.android_wire_bind_device(&other, otherdevice).await?;
    assert!(matches!(
        store.android_personal_project(&other, project).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .android_wire_sync_push(
                &other,
                vec![change(
                    otherdevice,
                    "project",
                    project,
                    0,
                    Some(json!({"title":"Cannot adopt","thread_ids":[thread]}))
                )]
            )
            .await,
        Err(Error::Forbidden)
    ));
    let independent_created = store
        .android_personal_create_project(
            &actor,
            json!({"privacy":"private_local","name":"Independent","description":"","policy":{}}),
        )
        .await?;
    let independent: Uuid = independent_created["id"].as_str().unwrap().parse()?;
    assert_eq!(independent_created["revision"], json!(1));
    assert_eq!(
        independent_created["owner_user_id"],
        json!(actor.identity_id())
    );
    assert!(independent_created["team_id"].is_null());
    assert_eq!(independent_created["privacy"], "private_local");
    assert!(independent_created["created_at"].as_str().is_some());
    assert!(independent_created["deleted_at"].is_null());
    assert_eq!(
        store
            .android_personal_project(&actor, independent)
            .await?
            .unwrap(),
        independent_created
    );
    let independent_wire = |page: Value| -> Value {
        let matches: Vec<_> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["entity_id"] == json!(independent))
            .collect();
        assert_eq!(matches.len(), 1);
        (*matches[0]).clone()
    };
    let wire_before = independent_wire(
        store
            .android_wire_sync_entities(&actor, "project", None, None, 100)
            .await?,
    );
    assert_eq!(wire_before["schema_version"], json!(2));
    assert_eq!(wire_before["entity_type"], "project");
    assert_eq!(wire_before["entity_id"], json!(independent));
    assert_eq!(wire_before["revision"], json!(1));
    assert_eq!(wire_before["tombstone"], json!(false));
    assert_eq!(wire_before["payload"], independent_created);
    // Use the legacy shape without contradictory canonical name/description aliases.
    let independent_payload = json!({
        "title":"API project updated through Wire",
        "instructions":"Roundtrip description through Wire",
        "policy":independent_created["policy"].clone(),
        "preferred_executor_target":independent_created["preferred_executor_target"].clone()
    });
    let independent_change = change(
        device,
        "project",
        independent,
        1,
        Some(independent_payload.clone()),
    );
    let applied = store
        .android_wire_sync_push(&actor, vec![independent_change.clone()])
        .await?;
    assert_eq!(applied["schema_version"], json!(2));
    let results = applied["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let receipt = &results[0];
    assert_eq!(receipt["schema_version"], json!(2));
    assert_eq!(
        receipt["operation_id"],
        json!(independent_change.operation_id)
    );
    assert_eq!(receipt["status"], "applied");
    assert_eq!(receipt["entity"]["schema_version"], json!(2));
    assert_eq!(receipt["entity"]["entity_type"], "project");
    assert_eq!(receipt["entity"]["entity_id"], json!(independent));
    assert_eq!(receipt["entity"]["revision"], json!(2));
    assert_eq!(receipt["entity"]["tombstone"], json!(false));
    assert_eq!(receipt["entity"]["payload"], independent_payload);
    let canonical_after = store
        .android_personal_project(&actor, independent)
        .await?
        .unwrap();
    let mut expected_after = independent_created.clone();
    expected_after["revision"] = json!(2);
    expected_after["etag"] = json!(format!("{independent}:2"));
    expected_after["name"] = independent_payload["title"].clone();
    expected_after["description"] = independent_payload["instructions"].clone();
    assert!(canonical_after["updated_at"].as_str().is_some());
    expected_after["updated_at"] = canonical_after["updated_at"].clone();
    assert_eq!(canonical_after, expected_after);
    assert_eq!(canonical_after["id"], json!(independent));
    assert_eq!(canonical_after["owner_user_id"], json!(actor.identity_id()));
    assert!(canonical_after["team_id"].is_null());
    assert_eq!(canonical_after["privacy"], "private_local");
    assert_eq!(
        canonical_after["created_at"],
        independent_created["created_at"]
    );
    assert!(canonical_after["deleted_at"].is_null());
    let wire_after = independent_wire(
        store
            .android_wire_sync_entities(&actor, "project", None, None, 100)
            .await?,
    );
    assert_eq!(wire_after, receipt["entity"]);
    assert_eq!(wire_after["updated_at"], canonical_after["updated_at"]);
    assert_eq!(
        store
            .android_wire_sync_push(&actor, vec![independent_change.clone()])
            .await?,
        applied
    );
    assert_eq!(
        store
            .android_personal_project(&actor, independent)
            .await?
            .unwrap(),
        canonical_after
    );
    assert_eq!(
        independent_wire(
            store
                .android_wire_sync_entities(&actor, "project", None, None, 100)
                .await?
        ),
        wire_after
    );
    let stale = change(
        device,
        "project",
        independent,
        1,
        Some(independent_payload.clone()),
    );
    assert_ne!(stale.operation_id, independent_change.operation_id);
    let stale_operation_id = stale.operation_id;
    let conflict = store.android_wire_sync_push(&actor, vec![stale]).await?;
    assert_eq!(
        conflict,
        json!({"schema_version":2,"results":[{
            "schema_version":2,"operation_id":stale_operation_id,"status":"conflict",
            "entity":wire_after.clone()
        }]})
    );
    assert_eq!(
        store
            .android_personal_project(&actor, independent)
            .await?
            .unwrap(),
        canonical_after
    );
    assert_eq!(
        independent_wire(
            store
                .android_wire_sync_entities(&actor, "project", None, None, 100)
                .await?
        ),
        wire_after
    );
    assert!(matches!(
        store.android_personal_project(&other, independent).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .android_wire_sync_push(
                &other,
                vec![change(
                    otherdevice,
                    "project",
                    independent,
                    2,
                    Some(independent_payload.clone())
                )]
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        store
            .android_personal_project(&actor, independent)
            .await?
            .unwrap(),
        canonical_after
    );
    assert_eq!(
        independent_wire(
            store
                .android_wire_sync_entities(&actor, "project", None, None, 100)
                .await?
        ),
        wire_after
    );
    let reverse = store
        .android_personal_update_project(
            &actor,
            independent,
            json!({
                "expected_revision":2,"name":"API reverse update of Wire project",
                "description":"Roundtrip description through API",
                "policy":canonical_after["policy"].clone(),
                "preferred_executor_target":canonical_after["preferred_executor_target"].clone()
            }),
            false,
        )
        .await?;
    let mut expected_reverse = canonical_after.clone();
    expected_reverse["revision"] = json!(3);
    expected_reverse["etag"] = json!(format!("{independent}:3"));
    expected_reverse["name"] = json!("API reverse update of Wire project");
    expected_reverse["description"] = json!("Roundtrip description through API");
    assert!(reverse["updated_at"].as_str().is_some());
    expected_reverse["updated_at"] = reverse["updated_at"].clone();
    assert_eq!(reverse, expected_reverse);
    assert_eq!(
        store
            .android_personal_project(&actor, independent)
            .await?
            .unwrap(),
        reverse
    );
    let reverse_wire = independent_wire(
        store
            .android_wire_sync_entities(&actor, "project", None, None, 100)
            .await?,
    );
    assert_eq!(reverse_wire["schema_version"], json!(2));
    assert_eq!(reverse_wire["entity_type"], "project");
    assert_eq!(reverse_wire["entity_id"], json!(independent));
    assert_eq!(reverse_wire["revision"], json!(3));
    assert_eq!(reverse_wire["tombstone"], json!(false));
    assert_eq!(reverse_wire["updated_at"], reverse["updated_at"]);
    let mut expected_reverse_payload = independent_payload.clone();
    expected_reverse_payload
        .as_object_mut()
        .unwrap()
        .extend(reverse.as_object().unwrap().clone());
    expected_reverse_payload["title"] = reverse["name"].clone();
    expected_reverse_payload["instructions"] = reverse["description"].clone();
    assert_eq!(reverse_wire["payload"], expected_reverse_payload);
    store.android_personal_update_project(&actor,project,json!({"expected_revision":1,"name":"Updated through API","description":"Preserve legacy shape","policy":{}}),false).await?;
    let page = store
        .android_wire_sync_entities(&actor, "project", None, None, 100)
        .await?;
    let synced = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["entity_id"] == json!(project))
        .unwrap();
    assert_eq!(synced["payload"]["title"], "Updated through API");
    assert_eq!(synced["payload"]["thread_ids"], json!([thread]));
    store
        .android_wire_sync_push(&actor, vec![change(device, "project", project, 2, None)])
        .await?;
    let feed = store.android_wire_sync_pull(&actor, 0, 100).await?;
    for (kind, id) in [
        ("project", project),
        ("thread", thread),
        ("task", task),
        ("message", message),
    ] {
        assert!(
            feed["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["entity_type"] == kind
                    && v["entity_id"] == json!(id)
                    && v["operation"] == "delete")
        );
    }
    assert!(store.android_tasks(&actor, project).await.is_err());
    assert!(
        store
            .android_personal_messages(&actor, thread)
            .await
            .is_err()
    );
    let key_id = cowork_secrets::KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    };
    let keys = SyntheticKey(key_id);
    let request = json!({"team_id":null,"name":"Actor model","provider_kind":"openai_compatible","model_defaults":{"base_url":"https://synthetic.invalid/v1/","model":"synthetic-model","auth_mode":"bearer"},"api_key":"Synthetic-private-key"});
    let original = store
        .android_personal_provider_mutate(
            &actor,
            "create",
            None,
            request.clone(),
            Some("provider-create"),
            &keys,
            key_id,
        )
        .await?;
    assert_eq!(
        store
            .android_personal_provider_mutate(
                &actor,
                "create",
                None,
                request,
                Some("provider-create"),
                &keys,
                key_id
            )
            .await?,
        original
    );
    let provider = Uuid::parse_str(original["id"].as_str().unwrap())?;
    let frozen = cowork_storage::android_personal_provider::binding(&original)?;
    let (_, secret) = store
        .android_personal_provider_dispatch(&actor, &frozen, &keys)
        .await?;
    assert_eq!(secret.unwrap().as_str(), "Synthetic-private-key");
    assert!(
        store
            .android_personal_provider(&other, provider)
            .await
            .is_err()
    );
    assert!(
        store
            .android_personal_provider_dispatch(&other, &frozen, &keys)
            .await
            .is_err()
    );
    let update = json!({"expected_revision":1,"name":"New exact endpoint","provider_kind":"openai_compatible","model_defaults":{"base_url":"https://new-synthetic.invalid/v1","model":"synthetic-model","auth_mode":"bearer"}});
    let changed = store
        .android_personal_provider_mutate(
            &actor,
            "update",
            Some(provider),
            update.clone(),
            Some("provider-update"),
            &keys,
            key_id,
        )
        .await?;
    let (retained, retained_secret) = store
        .android_personal_provider_dispatch(&actor, &frozen, &keys)
        .await?;
    assert_eq!(retained, original);
    assert_eq!(
        cowork_storage::android_personal_provider::binding(&retained)?,
        frozen
    );
    assert_eq!(retained["revision"], 1);
    assert_eq!(
        retained["model_defaults"]["base_url"],
        "https://synthetic.invalid/v1"
    );
    assert_eq!(retained["model_defaults"]["model"], "synthetic-model");
    assert!(retained_secret.unwrap().as_str() == "Synthetic-private-key");
    assert_eq!(
        store
            .android_personal_provider_mutate(
                &actor,
                "update",
                Some(provider),
                update,
                Some("provider-update"),
                &keys,
                key_id
            )
            .await?,
        changed
    );
    let changed_binding = cowork_storage::android_personal_provider::binding(&changed)?;
    let (updated, secret) = store
        .android_personal_provider_dispatch(&actor, &changed_binding, &keys)
        .await?;
    assert_eq!(updated, changed);
    assert_eq!(
        cowork_storage::android_personal_provider::binding(&updated)?,
        changed_binding
    );
    assert_eq!(updated["revision"], 2);
    assert_eq!(
        updated["model_defaults"]["base_url"],
        "https://new-synthetic.invalid/v1"
    );
    assert_eq!(updated["model_defaults"]["model"], "synthetic-model");
    assert_eq!(secret.unwrap().as_str(), "Synthetic-private-key");
    assert_ne!(frozen, changed_binding);
    assert_ne!(frozen["sha256"], changed_binding["sha256"]);
    for (bound, alternate) in [(&frozen, &changed_binding), (&changed_binding, &frozen)] {
        assert!(matches!(
            store
                .android_personal_provider_dispatch(&other, bound, &keys)
                .await,
            Err(Error::Forbidden)
        ));
        let mut forged_sha = (*bound).clone();
        forged_sha["sha256"] = alternate["sha256"].clone();
        assert!(matches!(
            store
                .android_personal_provider_dispatch(&actor, &forged_sha, &keys)
                .await,
            Err(Error::Forbidden)
        ));
        let mut mixed_endpoint = (*bound).clone();
        mixed_endpoint["endpoint"] = alternate["endpoint"].clone();
        assert!(matches!(
            store
                .android_personal_provider_dispatch(&actor, &mixed_endpoint, &keys)
                .await,
            Err(Error::Forbidden)
        ));
    }
    assert!(matches!(
        store
            .android_personal_provider_mutate(
                &actor,
                "secret",
                Some(provider),
                json!({"expected_revision":2,"api_key":"Different-bytes"}),
                Some("provider-update"),
                &keys,
                key_id
            )
            .await,
        Err(Error::Conflict)
    ));
    let removed = store
        .android_personal_provider_mutate(
            &actor,
            "delete",
            Some(provider),
            json!({"expected_revision":2}),
            Some("provider-delete"),
            &keys,
            key_id,
        )
        .await?;
    assert_eq!(
        store
            .android_personal_provider_mutate(
                &actor,
                "delete",
                Some(provider),
                json!({"expected_revision":2}),
                Some("provider-delete"),
                &keys,
                key_id
            )
            .await?,
        removed
    );
    for bound in [&frozen, &changed_binding] {
        assert!(matches!(
            store
                .android_personal_provider_dispatch(&actor, bound, &keys)
                .await,
            Err(Error::Forbidden)
        ));
    }
    println!(
        "PASS: out-of-order canonical projection, original receipts, immutable operation bytes, owner/ID isolation, legacy API merge, dependent tombstones and inert history"
    );
    Ok(())
}
