//! Real synthetic SQLite tests. All fixture input reads use ordinary byte readers;
//! SQLite only receives private generated copies. No product/live DB is accessed.
use super::*;
use serde_json::json;
use std::io::Cursor;

struct Buffers {
    q: Vec<u8>,
    db: Vec<u8>,
    mapping: Vec<u8>,
    preview: Vec<u8>,
    target: Vec<u8>,
    pins: HistoricalPins,
    original_source: Vec<u8>,
}
fn fixture(name: &str) -> Buffers {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/v1_history_verifier")
        .join(name);
    let bundle: Value =
        serde_json::from_slice(&std::fs::read(root.join("bundle.json")).unwrap()).unwrap();
    let read = |field: &str| {
        std::fs::read(root.join(bundle[field].as_str().unwrap().replace('\\', "/"))).unwrap()
    };
    let pin = |field: &str| Sha256Pin::parse(bundle["pins"][field].as_str().unwrap()).unwrap();
    Buffers {
        q: read("quarantine_manifest"),
        db: read("quarantine_database"),
        mapping: read("mapping"),
        preview: read("preview"),
        target: read("cold_target_snapshot"),
        pins: HistoricalPins {
            quarantine_manifest: pin("quarantine_manifest"),
            quarantine_database: pin("quarantine_database"),
            export_manifest: pin("export_manifest"),
            mapping: pin("mapping"),
            preview: pin("preview"),
            cold_target_snapshot: pin("cold_target_snapshot"),
        },
        original_source: std::fs::read(root.join("inputs/source.sqlite3")).unwrap(),
    }
}
impl Buffers {
    fn streams(&self) -> UntrustedHistoricalStreams {
        UntrustedHistoricalStreams {
            quarantine_manifest: Box::new(Cursor::new(self.q.clone())),
            quarantine_database: Box::new(Cursor::new(self.db.clone())),
            mapping: Box::new(Cursor::new(self.mapping.clone())),
            preview: Box::new(Cursor::new(self.preview.clone())),
            cold_target_snapshot: Box::new(Cursor::new(self.target.clone())),
        }
    }
    async fn verify(&self) -> Result<VerifiedHistoryArchive> {
        verify(self.streams(), self.pins.clone()).await
    }
    fn reseal_db(&mut self) {
        let mut q = c::parse(&self.q, MAX_MANIFEST).unwrap();
        q["database"]["bytes"] = json!(self.db.len());
        q["database"]["sha256"] = json!(c::hex(&c::hash(&self.db)));
        seal(&mut q);
        self.q = c::canonical(&q).unwrap();
        self.q.push(b'\n');
        self.pins.quarantine_database = Sha256Pin(c::hex(&c::hash(&self.db)));
        self.pins.quarantine_manifest = Sha256Pin(c::hex(&c::hash(&self.q)));
        let mut preview = c::parse(&self.preview, MAX_PREVIEW).unwrap();
        preview["quarantine_sha256"] = json!(self.pins.quarantine_manifest.0);
        seal(&mut preview);
        self.preview = c::canonical(&preview).unwrap();
        self.preview.push(b'\n');
        self.pins.preview = Sha256Pin(c::hex(&c::hash(&self.preview)));
    }
    fn remap(&mut self, value: Value) {
        self.mapping = c::canonical(&value).unwrap();
        self.mapping.push(b'\n');
        self.pins.mapping = Sha256Pin(c::hex(&c::hash(&self.mapping)));
    }
    fn repreview(&mut self, mut value: Value) {
        seal(&mut value);
        self.preview = c::canonical(&value).unwrap();
        self.preview.push(b'\n');
        self.pins.preview = Sha256Pin(c::hex(&c::hash(&self.preview)));
    }
}
fn seal(value: &mut Value) {
    value.as_object_mut().unwrap().remove("manifest_sha256");
    let hash = c::hash_value(value).unwrap();
    value["manifest_sha256"] = json!(hash);
}
async fn mutate(database: &[u8], sql: &str) -> Vec<u8> {
    let stage = tempfile::tempdir().unwrap();
    let path = stage.path().join("synthetic-copy.sqlite3");
    std::fs::write(&path, database).unwrap();
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .foreign_keys(false);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
    std::fs::read(path).unwrap()
}
async fn refused(buffers: &Buffers, reason: &str) {
    match buffers.verify().await {
        Err(error) => assert!(
            error.to_string().contains(reason),
            "Unexpected refusal: {error:?}"
        ),
        Ok(_) => panic!("Tampered history accepted: {reason}"),
    }
}
async fn alter_original(buffers: &mut Buffers, table: &str, column: &str, replacement: Value) {
    // Construct a fully internally resealed adversarial export/quarantine. This
    // must be refused by original type/ID/project semantics, not a stale SHA.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("adversarial.sqlite3");
    std::fs::write(&path, &buffers.db).unwrap();
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(false),
    )
    .await
    .unwrap();
    let raw: Vec<u8> = sqlx::query_scalar("SELECT export_manifest FROM quarantine_meta")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    let mut export = c::parse(&raw, MAX_MANIFEST).unwrap();
    let columns = export["content"][table]["columns"].as_array().unwrap();
    let at = columns.iter().position(|c| c == column).unwrap();
    let rows = sqlx::query(
        "SELECT source_rowid,canonical_row FROM history_rows WHERE table_name=? ORDER BY position",
    )
    .bind(table)
    .fetch_all(&mut conn)
    .await
    .unwrap();
    let mut all = Vec::new();
    for (position, row) in rows.iter().enumerate() {
        let raw: Vec<u8> = row.try_get(1).unwrap();
        let mut value = c::parse(&raw, MAX_LINE).unwrap();
        if position == 0 {
            value["cells"][at] = replacement.clone();
        }
        let mut raw = c::canonical(&value).unwrap();
        raw.push(b'\n');
        all.extend(&raw);
        sqlx::query("UPDATE history_rows SET canonical_row=?,row_sha256=? WHERE table_name=? AND source_rowid=?").bind(&raw).bind(c::hex(&c::hash(&raw))).bind(table).bind(row.try_get::<i64,_>(0).unwrap()).execute(&mut conn).await.unwrap();
    }
    let table_sha = c::hex(&c::hash(&all));
    export["content"][table]["sha256"] = json!(table_sha);
    export["content"][table]["bytes"] = json!(all.len());
    export["content_sha256"] = json!(c::hash_value(&export["content"]).unwrap());
    seal(&mut export);
    let mut encoded = c::canonical(&export).unwrap();
    encoded.push(b'\n');
    let sha = c::hex(&c::hash(&encoded));
    sqlx::query("UPDATE source_tables SET content_sha256=? WHERE name=?")
        .bind(table_sha)
        .bind(table)
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE quarantine_meta SET export_manifest=?,export_sha256=?")
        .bind(encoded)
        .bind(&sha)
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    buffers.db = std::fs::read(path).unwrap();
    let mut manifest = c::parse(&buffers.q, MAX_MANIFEST).unwrap();
    manifest["export_sha256"] = json!(sha);
    seal(&mut manifest);
    buffers.q = c::canonical(&manifest).unwrap();
    buffers.q.push(b'\n');
    buffers.pins.export_manifest = Sha256Pin(sha.clone());
    buffers.reseal_db();
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["export_sha256"] = json!(sha);
    buffers.remap(mapping);
    let mut preview = c::parse(&buffers.preview, MAX_PREVIEW).unwrap();
    preview["export_sha256"] = json!(sha);
    preview["mapping_sha256"] = json!(buffers.pins.mapping.0);
    buffers.repreview(preview);
}

#[tokio::test]
async fn all_34_exact_original_profiles_and_new_known_singleton_are_verified() {
    let fixed = profiles::fixed().unwrap();
    assert_eq!(fixed["sources"].as_array().unwrap().len(), 34);
    for profile in fixed["sources"].as_array().unwrap() {
        let name = profile["profile"].as_str().unwrap();
        let buffers = fixture(name);
        let archive = buffers
            .verify()
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(archive.provenance().source_profile, name);
        assert_eq!(
            archive.provenance().verifier_contract,
            "localai-cowork-rust-inert-history/1-installation-singleton"
        );
        let mut rows = archive.rows().await.unwrap();
        let mut count = 0;
        while rows.next().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, archive.provenance().original_rows);
        rows.close().await.unwrap();
        let path = archive.stage.directory.path().to_owned();
        drop(archive);
        assert!(!path.exists(), "Private Stage must close naturally");
        let again = fixture(name);
        assert_eq!(buffers.original_source, again.original_source);
        assert_eq!(buffers.target, again.target);
        assert_eq!(buffers.db, again.db);
    }
}
#[tokio::test]
async fn rich_original_types_unknown_run_ids_and_persona_remain_inert() {
    let buffers = fixture("rich-desktop");
    let archive = buffers.verify().await.unwrap();
    assert_eq!(archive.mapping().len(), 3);
    let mut rows = archive.rows().await.unwrap();
    let (mut unknown, mut invalid_utf8, mut blob, mut minimum, mut maximum, mut real) =
        (false, false, false, false, false, false);
    while let Some(row) = rows.next().await.unwrap() {
        for cell in row.cells {
            match cell {
                HistoricalCell::TextBytes(v) => {
                    unknown |= v == b"old_future_state";
                    invalid_utf8 |= v == [0x80, 0xff, 0, 0x61];
                }
                HistoricalCell::Blob(v) => blob |= v == [0, 0x80, 0xff],
                HistoricalCell::Integer(v) => {
                    minimum |= v == i64::MIN;
                    maximum |= v == i64::MAX;
                }
                HistoricalCell::RealBits(v) => real |= v == 0.125f64.to_bits(),
                HistoricalCell::Null => {}
            }
        }
    }
    assert!(unknown && invalid_utf8 && blob && minimum && maximum && real);
    rows.close().await.unwrap();
    let mut projections = archive.projections().await.unwrap();
    let (mut persona, mut run, mut relation) = (false, false, false);
    while let Some(value) = projections.next().await.unwrap() {
        match value {
            HistoricalProjection::Entity(entity) if entity.kind == EntityKind::AgentPersona => {
                assert_eq!(entity.original_id, "old-persona");
                persona = true;
            }
            HistoricalProjection::Entity(entity) if entity.kind == EntityKind::EngineRun => {
                assert_eq!(entity.original_id, "old-run");
                assert_eq!(entity.original_project_id.as_deref(), Some("old-project"));
                run = true;
            }
            HistoricalProjection::Relation(value) => {
                relation |= value.kind == HistoricalRelationKind::RunArtifact
            }
            _ => {}
        }
    }
    assert!(persona && run && relation);
    projections.close().await.unwrap();
    for mapping in archive.mapping() {
        if let Some(value) = mapping.historical_cold_target_facts() {
            let data = c::canonical(value).unwrap();
            assert!(!String::from_utf8(data).unwrap().contains("DO_NOT_EXPORT"));
        }
    }
}
#[tokio::test]
async fn daemon_record_spec_actor_project_and_unknown_run_are_historical() {
    let buffers = fixture("rich-daemon");
    let archive = buffers.verify().await.unwrap();
    let mut records = archive.projections().await.unwrap();
    let (mut actor, mut run) = (false, false);
    while let Some(value) = records.next().await.unwrap() {
        match value {
            HistoricalProjection::Actor(value) => {
                actor |= value.original_id == "50000000-0000-0000-0000-000000000001"
            }
            HistoricalProjection::Entity(value) if value.kind == EntityKind::DaemonRun => {
                assert_eq!(value.original_id, "old-daemon-run");
                assert_eq!(
                    value.original_actor_id.as_deref(),
                    Some("50000000-0000-0000-0000-000000000001")
                );
                run = true;
            }
            _ => {}
        }
    }
    assert!(actor && run);
    records.close().await.unwrap();
}
#[tokio::test]
async fn exact_tenant_resource_creator_and_unicode_creation_receipt_are_pins() {
    let buffers = fixture("rich-tenant");
    let archive = buffers.verify().await.unwrap();
    assert_eq!(archive.mapping().len(), 1);
    assert!(matches!(
        archive.mapping()[0].namespace(),
        HistoryNamespace::Tenant { .. }
    ));
}
#[tokio::test]
async fn explicit_byte_pins_and_unique_structural_json_refuse_tamper() {
    let mut buffers = fixture("rich-desktop");
    buffers.db[100] ^= 1;
    refused(&buffers, "Explicit historical byte pin mismatch").await;
    let mut buffers = fixture("rich-desktop");
    buffers.mapping = b"{\"format\":1,\"format\":2}".to_vec();
    buffers.pins.mapping = Sha256Pin(c::hex(&c::hash(&buffers.mapping)));
    refused(&buffers, "Invalid/duplicate/deep JSON").await;
}
#[tokio::test]
async fn resealed_original_rows_and_projects_cannot_replace_original_export() {
    let mut buffers = fixture("rich-desktop");
    buffers.db=mutate(&buffers.db,"UPDATE history_rows SET canonical_row=x'7b7d0a',row_sha256='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' WHERE table_name='engine_runs'").await;
    buffers.reseal_db();
    refused(&buffers, "Original row position/SHA differs").await;
    let mut buffers = fixture("rich-desktop");
    buffers.db = mutate(
        &buffers.db,
        "UPDATE history_entities SET project_source_id='old-other-project' WHERE kind='engine_run'",
    )
    .await;
    buffers.reseal_db();
    refused(&buffers, "projection differs").await;
}
#[tokio::test]
async fn complete_known_source_and_target_profiles_reject_unknown_objects() {
    let mut buffers = fixture("rich-desktop");
    buffers.db = mutate(&buffers.db, "CREATE TABLE unknown(x)").await;
    buffers.reseal_db();
    refused(&buffers, "schema objects").await;
    let mut buffers = fixture("rich-desktop");
    buffers.target=mutate(&buffers.target,"CREATE TABLE accounting_personal_configuration(id TEXT PRIMARY KEY,payload_sha256 TEXT NOT NULL,payload TEXT NOT NULL)").await;
    buffers.pins.cold_target_snapshot = Sha256Pin(c::hex(&c::hash(&buffers.target)));
    refused(&buffers, "schema objects").await;
}
#[tokio::test]
async fn snapshot_migrations_and_known_installation_singleton_are_exact() {
    let mut buffers = fixture("rich-desktop");
    buffers.target = mutate(
        &buffers.target,
        "UPDATE schema_migrations SET checksum='bad' WHERE version=20",
    )
    .await;
    buffers.pins.cold_target_snapshot = Sha256Pin(c::hex(&c::hash(&buffers.target)));
    refused(&buffers, "migration").await;
    let mut buffers = fixture("rich-desktop");
    buffers.target = mutate(
        &buffers.target,
        "PRAGMA ignore_check_constraints=ON;UPDATE identity_installation SET id=2",
    )
    .await;
    buffers.pins.cold_target_snapshot = Sha256Pin(c::hex(&c::hash(&buffers.target)));
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["target_snapshot_sha256"] = json!(buffers.pins.cold_target_snapshot.0);
    buffers.remap(mapping);
    refused(&buffers, "INTEGER id=1").await;
}
#[tokio::test]
async fn duplicate_multivalued_mapping_and_crossproject_target_ids_are_refused() {
    let mut buffers = fixture("rich-desktop");
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    let first = mapping["entries"][0].clone();
    mapping["entries"].as_array_mut().unwrap().push(first);
    buffers.remap(mapping);
    refused(&buffers, "Duplicate/multivalued").await;
    let mut buffers = fixture("rich-desktop");
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["entries"][1]["project_id"] = json!("10000000-0000-0000-0000-000000000002");
    buffers.remap(mapping);
    refused(&buffers, "another project/namespace").await;
}
#[tokio::test]
async fn preview_is_rederived_and_cannot_add_credential_grant_or_execution_authority() {
    let mut buffers = fixture("rich-desktop");
    let mut preview = c::parse(&buffers.preview, MAX_PREVIEW).unwrap();
    preview["diff"]["entries"][0]["grants_created"] = json!(1);
    buffers.repreview(preview);
    refused(&buffers, "Preview provenance/diff/policy differs").await;
    let mut buffers = fixture("rich-desktop");
    let mut preview = c::parse(&buffers.preview, MAX_PREVIEW).unwrap();
    preview["activation_allowed"] = json!(true);
    buffers.repreview(preview);
    refused(&buffers, "Preview provenance/diff/policy differs").await;
}
#[tokio::test]
async fn fully_resealed_original_nullable_identity_and_crossproject_parent_are_refused() {
    let mut buffers = fixture("rich-desktop");
    alter_original(
        &mut buffers,
        "agent_personalities",
        "id",
        json!({"type":"null"}),
    )
    .await;
    refused(&buffers, "Duplicate/null/ambiguously typed original ID").await;
    let mut buffers = fixture("rich-parent");
    alter_original(
        &mut buffers,
        "engine_runs",
        "parent_run_id",
        c::encoded_cell(&HistoricalCell::TextBytes(b"old-run".to_vec())),
    )
    .await;
    refused(&buffers, "Original parent run crosses project").await;
}
#[tokio::test]
async fn fully_resealed_daemon_spec_cannot_change_actor_project_or_run_id() {
    let mut buffers = fixture("rich-daemon");
    let record = json!({"id":"different-run","spec":{"project_id":"10000000-0000-0000-0000-000000000001","creator_user_id":"50000000-0000-0000-0000-000000000001"}});
    alter_original(
        &mut buffers,
        "daemon_runs",
        "record_json",
        c::encoded_cell(&HistoricalCell::TextBytes(c::canonical(&record).unwrap())),
    )
    .await;
    refused(&buffers, "RunRecord ID differs").await;
    let mut buffers = fixture("rich-daemon");
    let record = json!({"id":"old-daemon-run","spec":{"project_id":"10000000-0000-0000-0000-000000000001","creator_user_id":"50000000-0000-0000-0000-000000000001","project":{"id":"10000000-0000-0000-0000-000000000002"}}});
    alter_original(
        &mut buffers,
        "daemon_runs",
        "record_json",
        c::encoded_cell(&HistoricalCell::TextBytes(c::canonical(&record).unwrap())),
    )
    .await;
    refused(&buffers, "project object conflicts").await;
}
#[tokio::test]
async fn persona_human_confusion_and_two_sources_to_same_target_are_refused() {
    let mut buffers = fixture("rich-desktop");
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["entries"] = json!([{"source_kind":"agent_persona","source_id":"old-persona","target_kind":"identity_subject","target_id":"50000000-0000-0000-0000-000000000001","namespace":"history","project_id":null,"organization_id":null,"creator_identity_id":null}]);
    buffers.remap(mapping);
    refused(&buffers, "persona mapping conflation").await;
    let mut buffers = fixture("rich-desktop");
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["entries"][1]["target_id"] = mapping["entries"][0]["target_id"].clone();
    buffers.remap(mapping);
    refused(&buffers, "Multiple original entities target same V2 ID").await;
}
#[tokio::test]
async fn unknown_original_id_and_incomplete_artifact_run_mapping_are_refused() {
    let mut buffers = fixture("rich-desktop");
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["entries"][1]["source_id"] = json!("unknown-original-id");
    buffers.remap(mapping);
    refused(&buffers, "absent original ID").await;
    let mut buffers = fixture("rich-desktop");
    let mut mapping = c::parse(&buffers.mapping, MAX_MANIFEST).unwrap();
    mapping["entries"].as_array_mut().unwrap().remove(1);
    buffers.remap(mapping);
    refused(&buffers, "Artifact original run mapping missing").await;
}
#[tokio::test]
async fn source_whole_schema_and_version_binding_cannot_be_resealed_into_known_profile() {
    let mut buffers = fixture("rich-desktop");
    let mut manifest = c::parse(&buffers.q, MAX_MANIFEST).unwrap();
    manifest["source_profile"] = json!("desktop-999");
    seal(&mut manifest);
    buffers.q = c::canonical(&manifest).unwrap();
    buffers.q.push(b'\n');
    buffers.pins.quarantine_manifest = Sha256Pin(c::hex(&c::hash(&buffers.q)));
    refused(&buffers, "source profile differs").await;
}
#[tokio::test]
async fn source_version_marker_is_rederived_from_complete_original_rows() {
    let mut buffers = fixture("rich-desktop");
    alter_original(
        &mut buffers,
        "schema_version",
        "version",
        c::encoded_cell(&HistoricalCell::Integer(27)),
    )
    .await;
    refused(&buffers, "Original schema version marker differs").await;
}
#[tokio::test]
async fn actual_sqlite_progress_deadline_interrupts_and_rollback_is_private() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let mut conn = connection(&stage, "cold-target.sqlite3", true, Budget::new())
        .await
        .unwrap();
    {
        let mut handle = conn.lock_handle().await.unwrap();
        let deadline = Instant::now() + Duration::from_millis(40);
        handle.set_progress_handler(1000, move || Instant::now() <= deadline);
    }
    let failure = sqlx::query(
        "WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n",
    )
    .fetch_one(&mut *conn)
    .await;
    assert!(failure.is_err());
    conn.close().await.unwrap();
    drop(stage);
    assert!(!path.exists());
}
#[tokio::test]
async fn canceled_sqlite_future_keeps_private_stage_until_actual_worker_close() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let mut conn = connection(&stage, "cold-target.sqlite3", true, Budget::new())
        .await
        .unwrap();
    {
        let mut handle = conn.lock_handle().await.unwrap();
        let deadline = Instant::now() + Duration::from_millis(80);
        handle.set_progress_handler(1000, move || Instant::now() <= deadline);
    }
    drop(stage);
    let task = tokio::spawn(async move {
        sqlx::query(
            "WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n",
        )
        .fetch_one(&mut *conn)
        .await
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    let end = Instant::now() + Duration::from_secs(3);
    while path.exists() && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !path.exists(),
        "Canceled private Stage cleanup must complete"
    );
}
#[tokio::test]
async fn cursor_lifetime_retains_private_bytes_then_closes_naturally() {
    let buffers = fixture("rich-desktop");
    let archive = buffers.verify().await.unwrap();
    let path = archive.stage.directory.path().to_owned();
    let mut rows = archive.rows().await.unwrap();
    drop(archive);
    assert!(path.exists());
    assert!(rows.next().await.unwrap().is_some());
    rows.close().await.unwrap();
    assert!(!path.exists());
}
#[cfg(windows)]
#[tokio::test]
async fn native_captured_file_and_namespace_fences_deny_write_delete_and_rename() {
    let buffers = fixture("rich-desktop");
    let archive = buffers.verify().await.unwrap();
    let path = archive.stage.path("quarantine.sqlite3");
    assert!(OpenOptions::new().write(true).open(&path).is_err());
    assert!(std::fs::remove_file(&path).is_err());
    assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
    let directory = archive.stage.directory.path();
    assert!(std::fs::rename(directory, directory.with_extension("moved")).is_err());
    let mut reader = connection(&archive.stage, "cold-target.sqlite3", true, Budget::new())
        .await
        .unwrap();
    assert!(
        sqlx::query("DELETE FROM identity_sessions")
            .execute(&mut *reader)
            .await
            .is_err()
    );
    reader.close().await.unwrap();
}
#[test]
fn typed_int64_float_binary_and_deep_duplicate_json_limits_are_strict() {
    assert!(c::decimal("-0").is_err());
    assert!(c::decimal("9223372036854775808").is_err());
    assert!(matches!(
        c::cell(&json!({"type":"real","ieee754_binary64_be":"7ff0000000000000"})),
        Err(Error::Refused("Nonfinite historical real"))
    ));
    let raw = format!("{}0{}", "[".repeat(34), "]".repeat(34));
    assert!(c::parse(raw.as_bytes(), MAX_MANIFEST).is_err());
    assert!(c::parse(b"{\"x\":1,\"x\":2}", MAX_MANIFEST).is_err());
}
#[test]
fn zzz_private_cleanup_workers_finish_before_qualification_process_exit() {
    let end = Instant::now() + Duration::from_secs(3);
    while (PRIVATE_CLOSE_OPERATIONS.load(std::sync::atomic::Ordering::SeqCst) > 0
        || PRIVATE_BUILD_OPERATIONS.load(std::sync::atomic::Ordering::SeqCst) > 0)
        && Instant::now() < end
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        PRIVATE_CLOSE_OPERATIONS.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        PRIVATE_BUILD_OPERATIONS.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}
async fn wait_private_stage_gone(path: &std::path::Path) {
    let end = Instant::now() + Duration::from_secs(3);
    while path.exists() && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !path.exists(),
        "Owned startup/close must finish before Stage release"
    );
}
#[tokio::test]
async fn canceled_memory_startup_receiver_retains_stage_until_independent_close_ack() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let (arrived, observe) = std::sync::mpsc::channel();
    let (resume, gate) = std::sync::mpsc::channel();
    let receiver = begin_connection(
        &stage,
        "cold-target.sqlite3",
        true,
        Budget::new(),
        Some(StartupHook {
            phase: StartupPhase::MemoryReady,
            arrived,
            resume: gate,
            fail_after_attach: false,
        }),
    )
    .unwrap();
    observe.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(stage);
    drop(receiver);
    assert!(path.exists());
    resume.send(()).unwrap();
    wait_private_stage_gone(&path).await;
}
#[tokio::test]
async fn canceled_attached_file_startup_receiver_retains_native_handle_until_close_ack() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let (arrived, observe) = std::sync::mpsc::channel();
    let (resume, gate) = std::sync::mpsc::channel();
    let receiver = begin_connection(
        &stage,
        "cold-target.sqlite3",
        true,
        Budget::new(),
        Some(StartupHook {
            phase: StartupPhase::AttachmentReady,
            arrived,
            resume: gate,
            fail_after_attach: false,
        }),
    )
    .unwrap();
    observe.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(stage);
    drop(receiver);
    assert!(path.exists());
    resume.send(()).unwrap();
    wait_private_stage_gone(&path).await;
}
#[tokio::test]
async fn initial_attached_database_sql_error_is_closed_with_ack_before_stage_cleanup() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let (arrived, observe) = std::sync::mpsc::channel();
    let (resume, gate) = std::sync::mpsc::channel();
    let receiver = begin_connection(
        &stage,
        "cold-target.sqlite3",
        true,
        Budget::new(),
        Some(StartupHook {
            phase: StartupPhase::AttachmentReady,
            arrived,
            resume: gate,
            fail_after_attach: true,
        }),
    )
    .unwrap();
    observe.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(stage);
    assert!(path.exists());
    resume.send(()).unwrap();
    assert!(matches!(receiver.await.unwrap(), Err(Error::Sqlite(_))));
    wait_private_stage_gone(&path).await;
}
#[tokio::test]
async fn corrupted_database_header_during_native_attach_is_closed_with_ack() {
    let mut buffers = fixture("rich-desktop");
    buffers.target.fill(0xff);
    buffers.pins.cold_target_snapshot = Sha256Pin(c::hex(&c::hash(&buffers.target)));
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let receiver =
        begin_connection(&stage, "cold-target.sqlite3", true, Budget::new(), None).unwrap();
    drop(stage);
    assert!(matches!(receiver.await.unwrap(), Err(Error::Sqlite(_))));
    wait_private_stage_gone(&path).await;
}
#[tokio::test]
async fn integrity_fk_and_version_probes_must_check_attached_private_schema() {
    let mut buffers = fixture("rich-desktop");
    buffers.db=mutate(&buffers.db,"UPDATE history_entities SET project_source_id='absent-original-project' WHERE kind='engine_run'").await;
    buffers.reseal_db();
    refused(&buffers, "Historical foreign key check failed").await;
    let mut buffers = fixture("rich-desktop");
    buffers.target = mutate(&buffers.target, "PRAGMA user_version=21").await;
    buffers.pins.cold_target_snapshot = Sha256Pin(c::hex(&c::hash(&buffers.target)));
    refused(&buffers, "target SQLite metadata version").await;
}
#[tokio::test]
async fn own_uri_percent_encoding_does_not_turn_path_bytes_into_sqlite_options() {
    let directory = tempfile::Builder::new()
        .prefix(".history # % ü space-")
        .tempdir()
        .unwrap();
    let path = directory.path().join("cold-target.sqlite3");
    std::fs::write(&path, &fixture("rich-desktop").target).unwrap();
    let stage = Arc::new(PrivateStage {
        files: Mutex::new(Vec::new()),
        _namespace: Vec::new(),
        input_receipts: Vec::new(),
        directory,
    });
    let uri = attachment_uri(&stage, "cold-target.sqlite3", true).unwrap();
    assert!(
        uri.contains("%23")
            && uri.contains("%25")
            && uri.contains("%20")
            && uri.ends_with("?mode=ro&immutable=1")
    );
    let mut conn = connection(&stage, "cold-target.sqlite3", true, Budget::new())
        .await
        .unwrap();
    let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM history_data.schema_migrations")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(actual, 20);
    assert!(
        sqlx::query("DELETE FROM history_data.schema_migrations")
            .execute(&mut *conn)
            .await
            .is_err()
    );
    conn.close().await.unwrap();
}
#[tokio::test]
async fn paused_startup_cannot_extend_the_original_verification_deadline() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let (arrived, observe) = std::sync::mpsc::channel();
    let (resume, gate) = std::sync::mpsc::channel();
    let receiver = begin_connection(
        &stage,
        "cold-target.sqlite3",
        true,
        Budget {
            deadline: Instant::now() + Duration::from_millis(100),
        },
        Some(StartupHook {
            phase: StartupPhase::MemoryReady,
            arrived,
            resume: gate,
            fail_after_attach: false,
        }),
    )
    .unwrap();
    observe.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(stage);
    tokio::time::sleep(Duration::from_millis(130)).await;
    resume.send(()).unwrap();
    assert!(matches!(
        receiver.await.unwrap(),
        Err(Error::Refused("Historical verification time limit"))
    ));
    wait_private_stage_gone(&path).await;
}
#[tokio::test]
async fn canceled_close_acknowledgment_cannot_release_stage_before_native_handle() {
    let buffers = fixture("rich-desktop");
    let stage = capture(buffers.streams(), &buffers.pins, Budget::new()).unwrap();
    let path = stage.directory.path().to_owned();
    let mut conn = connection(&stage, "cold-target.sqlite3", true, Budget::new())
        .await
        .unwrap();
    {
        let mut handle = conn.lock_handle().await.unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        handle.set_progress_handler(1000, move || Instant::now() <= deadline);
    }
    drop(stage);
    // Leave one real VM query in the SQLx worker by canceling only its receiver.
    let mut query = Box::pin(
        sqlx::query(
            "WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n",
        )
        .fetch_one(&mut *conn),
    );
    tokio::select! {_=&mut query=>panic!("Unbounded VM unexpectedly completed"),_=tokio::time::sleep(Duration::from_millis(5))=>{}}
    drop(query);
    let task = tokio::spawn(async move { conn.close().await });
    tokio::time::sleep(Duration::from_millis(5)).await;
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    let end = Instant::now() + Duration::from_secs(3);
    while path.exists() && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!path.exists());
}
