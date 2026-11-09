//! Existing-project replacement and three-way merge use the same immutable copy admission.
use super::*;
use cowork_contracts::{
    AndroidApplyProjectMerge, AndroidApplyProjectVersion, AndroidMergeFileStatus,
    AndroidMergeResolutionChoice, AndroidProjectMergeReview,
};

pub(crate) async fn quiescence(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_project_copy_plans WHERE state IN('prepared','receiving','uncertain')").fetch_one(&mut **tx).await.map_err(storage_error)?;
    if count != 0 {
        return Err(Error::UncertainEffect);
    }
    Ok(())
}
pub(super) async fn head(tx: &mut Transaction<'_, Any>, project: Uuid) -> Result<Option<Uuid>> {
    sqlx::query_scalar::<_, String>(
        "SELECT current_version_id FROM android_project_version_heads WHERE project_id=$1",
    )
    .bind(project.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .map(|v| Uuid::parse_str(&v).map_err(storage_error))
    .transpose()
}
async fn target_digest(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
) -> Result<String> {
    let p = scope(store, tx, actor, project, ContentAction::Write).await?;
    let mut digest = Sha256::new();
    digest.update(json_string(&p.payload)?.as_bytes());
    let tables: Vec<(&str, &str)> = if p.team.is_none() {
        vec![
        ("thread","SELECT id,payload FROM android_personal_threads WHERE project_id=$1 AND deleted=0 AND id>$2 ORDER BY id LIMIT 1"),
        ("message","SELECT id,payload FROM android_personal_messages WHERE project_id=$1 AND deleted=0 AND id>$2 ORDER BY id LIMIT 1"),
        ("file","SELECT id,payload FROM android_private_files WHERE project_id=$1 AND deleted=0 AND id>$2 ORDER BY id LIMIT 1"),
    ]
    } else {
        vec![
        ("thread","SELECT id,payload FROM team_orders WHERE project_id=$1 AND id>$2 ORDER BY id LIMIT 1"),
        ("document","SELECT id,payload FROM team_documents WHERE project_id=$1 AND id>$2 ORDER BY id LIMIT 1"),
        ("file","SELECT id,payload FROM team_blobs WHERE project_id=$1 AND id>$2 ORDER BY id LIMIT 1"),
    ]
    };
    let mut total = 0u64;
    let mut count = 0usize;
    for (kind, sql) in tables {
        let mut after = String::new();
        loop {
            let Some(r) = sqlx::query(sql)
                .bind(project.to_string())
                .bind(&after)
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
            else {
                break;
            };
            after = r.try_get("id").map_err(storage_error)?;
            let encoded: String = r.try_get("payload").map_err(storage_error)?;
            total = total
                .checked_add(encoded.len() as u64)
                .ok_or(Error::BudgetExceeded)?;
            count += 1;
            if total > content::MAX_CONTENT || count > 20000 || encoded.len() > content::MAX_OBJECT
            {
                return Err(Error::BudgetExceeded);
            }
            digest.update(json_string(&(kind, &after, hash(encoded.as_bytes())))?.as_bytes());
        }
    }
    let mut after = String::new();
    loop {
        let Some(r)=sqlx::query("SELECT id,revision,payload FROM android_task_versions t WHERE project_id=$1 AND id>$2 AND revision=(SELECT MAX(v.revision) FROM android_task_versions v WHERE v.id=t.id) ORDER BY id LIMIT 1").bind(project.to_string()).bind(&after).fetch_optional(&mut **tx).await.map_err(storage_error)?else{break};
        after = r.try_get("id").map_err(storage_error)?;
        let encoded: String = r.try_get("payload").map_err(storage_error)?;
        total = total
            .checked_add(encoded.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if total > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
        digest.update(
            json_string(&(
                "task",
                &after,
                r.try_get::<i64, _>("revision").map_err(storage_error)?,
                hash(encoded.as_bytes()),
            ))?
            .as_bytes(),
        );
    }
    let mut after = (String::new(), String::new(), String::new());
    loop {
        let Some(r)=sqlx::query("SELECT owner_id,entity_type,entity_id,digest FROM android_wire_sync_current WHERE project_id=$1 AND (owner_id>$2 OR (owner_id=$2 AND entity_type>$3) OR (owner_id=$2 AND entity_type=$3 AND entity_id>$4)) ORDER BY owner_id,entity_type,entity_id LIMIT 1").bind(project.to_string()).bind(&after.0).bind(&after.1).bind(&after.2).fetch_optional(&mut **tx).await.map_err(storage_error)?else{break};
        after = (
            r.try_get("owner_id").map_err(storage_error)?,
            r.try_get("entity_type").map_err(storage_error)?,
            r.try_get("entity_id").map_err(storage_error)?,
        );
        digest.update(
            json_string(&(
                &after,
                r.try_get::<String, _>("digest").map_err(storage_error)?,
            ))?
            .as_bytes(),
        );
    }
    scope(store, tx, actor, project, ContentAction::Write).await?;
    Ok(format!("{:x}", digest.finalize()))
}

async fn file_catalog(
    tx: &mut Transaction<'_, Any>,
    version: Uuid,
) -> Result<BTreeMap<String, (Uuid, u64, String, String, Value)>> {
    let rows=sqlx::query("SELECT path,source_id,size,sha256,media_type,payload FROM android_project_version_files WHERE version_id=$1 ORDER BY path LIMIT 2001").bind(version.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 2000 {
        return Err(Error::BudgetExceeded);
    }
    let mut out = BTreeMap::new();
    for r in rows {
        let path: String = r.try_get("path").map_err(storage_error)?;
        let bytes: String = r.try_get("payload").map_err(storage_error)?;
        if bytes.len() > 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        out.insert(
            path,
            (
                row_id(&r, "source_id")?,
                u64::try_from(r.try_get::<i64, _>("size").map_err(storage_error)?)
                    .map_err(|_| Error::Forbidden)?,
                r.try_get("sha256").map_err(storage_error)?,
                r.try_get("media_type").map_err(storage_error)?,
                parse(&bytes)?,
            ),
        );
    }
    Ok(out)
}
async fn file_text(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    s: &content::Scope,
    f: Option<&(Uuid, u64, String, String, Value)>,
) -> Result<Option<Vec<u8>>> {
    let Some(f) = f.filter(|f| f.1 <= merge::MAX_MERGE_TEXT_BYTES as u64) else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    for i in 0..f.1.div_ceil(content::PAGE as u64) {
        bytes.extend_from_slice(
            &content::read_page(tx, keys, s, &format!("file:{}", f.0), i as i64).await?,
        );
    }
    if bytes.len() as u64 != f.1 || hash(&bytes) != f.2 {
        return Err(Error::Forbidden);
    }
    Ok(Some(bytes))
}
async fn metadata(
    tx: &mut Transaction<'_, Any>,
    id: Uuid,
) -> Result<BTreeMap<(String, Uuid), String>> {
    let rows=sqlx::query("SELECT kind,source_id,payload_sha256 FROM android_project_version_objects WHERE version_id=$1 AND kind NOT LIKE 'origin:%' ORDER BY kind,source_id LIMIT 10001").bind(id.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    rows.into_iter()
        .map(|r| {
            Ok((
                (
                    r.try_get("kind").map_err(storage_error)?,
                    row_id(&r, "source_id")?,
                ),
                r.try_get("payload_sha256").map_err(storage_error)?,
            ))
        })
        .collect()
}
fn choice(
    analysis: &merge::Analysis,
    explicit: Option<AndroidMergeResolutionChoice>,
) -> Result<AndroidMergeResolutionChoice> {
    use AndroidMergeResolutionChoice::*;
    if let Some(v) = explicit {
        if v == AutoMerged && analysis.merged.is_none() {
            return Err(Error::Conflict);
        }
        return Ok(v);
    }
    Ok(match analysis.review.status {
        AndroidMergeFileStatus::TextConflict | AndroidMergeFileStatus::BinaryConflict => {
            return Err(Error::Conflict)
        }
        AndroidMergeFileStatus::AutoMerged => AutoMerged,
        AndroidMergeFileStatus::CurrentOnly => Current,
        AndroidMergeFileStatus::Deleted => Delete,
        AndroidMergeFileStatus::Unchanged | AndroidMergeFileStatus::IdenticalChange => Current,
        _ => Result,
    })
}
async fn merge_archive(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    actor: &Principal,
    p: &ProjectScope,
    body: &AndroidApplyProjectMerge,
    snapshot: Uuid,
    current: i64,
) -> Result<Uuid> {
    let (_, b, _) = version(tx, p.project, body.base_version_id).await?;
    let (_, c, _) = version(tx, p.project, body.current_version_id).await?;
    let (_, r, _) = version(tx, p.project, body.result_version_id).await?;
    let base = file_catalog(tx, b.version).await?;
    let cur = file_catalog(tx, c.version).await?;
    let res = file_catalog(tx, r.version).await?;
    let bm = metadata(tx, b.version).await?;
    let cm = metadata(tx, c.version).await?;
    let rm = metadata(tx, r.version).await?;
    let mut choices = BTreeMap::new();
    for selection in &body.resolutions {
        if choices
            .insert(selection.path.clone(), selection.choice)
            .is_some()
        {
            return Err(device_invalid());
        }
    }
    let revision: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(revision),0)+1 FROM android_project_versions WHERE project_id=$1",
    )
    .bind(p.project.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if revision > 1000 {
        return Err(Error::BudgetExceeded);
    }
    let id = Uuid::new_v4();
    let dest = content::Scope {
        owner: actor.identity_id(),
        project: p.project,
        version: id,
    };
    let value = AndroidProjectVersion {
        schema_version: 2,
        id,
        project_id: p.project,
        revision,
        parent_version_id: Some(c.version),
        merge_base_version_id: Some(b.version),
        snapshot_manifest_id: snapshot,
        created_by_run_id: None,
        created_at: device_iso(current)?,
    };
    let encoded = json_string(&value)?;
    sqlx::query("INSERT INTO android_project_versions(id,project_id,team_id,creator_id,revision,payload,payload_sha256,content_sha256,created_at,ready) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,0)").bind(id.to_string()).bind(p.project.to_string()).bind(p.team.map(|v|v.to_string())).bind(actor.identity_id().to_string()).bind(revision).bind(&encoded).bind(hash(encoded.as_bytes())).bind(hash(b"")).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let object_keys: BTreeSet<_> = bm
        .keys()
        .chain(cm.keys())
        .chain(rm.keys())
        .cloned()
        .collect();
    for (kind, source_id) in object_keys {
        let path = format!("@content/{kind}/{source_id}");
        let analysis = merge::analyze(
            &path,
            bm.get(&(kind.clone(), source_id)).map(String::as_str),
            cm.get(&(kind.clone(), source_id)).map(String::as_str),
            rm.get(&(kind.clone(), source_id)).map(String::as_str),
            None,
        );
        let pick = choice(&analysis, choices.remove(&path))?;
        let src = match pick {
            AndroidMergeResolutionChoice::Delete => continue,
            AndroidMergeResolutionChoice::Current => &c,
            AndroidMergeResolutionChoice::Result => &r,
            _ => return Err(Error::Conflict),
        };
        let object = content::read_object(tx, keys, src, &kind, source_id).await?;
        let raw = json_string(&object)?;
        total = total
            .checked_add(raw.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if total > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
        content::write_object(tx, keys, key, &dest, &kind, source_id, &object).await?;
        digest.update(json_string(&(&kind, source_id, hash(raw.as_bytes())))?.as_bytes());
    }
    let paths: BTreeSet<_> = base
        .keys()
        .chain(cur.keys())
        .chain(res.keys())
        .cloned()
        .collect();
    let mut file_bytes = 0u64;
    for path in paths {
        let bf = base.get(&path);
        let cf = cur.get(&path);
        let rf = res.get(&path);
        let bt = file_text(tx, keys, &b, bf).await?;
        let ct = file_text(tx, keys, &c, cf).await?;
        let rt = file_text(tx, keys, &r, rf).await?;
        let raw = bt
            .as_deref()
            .zip(ct.as_deref())
            .zip(rt.as_deref())
            .map(|((b, c), r)| (b, c, r));
        let analysis = merge::analyze(
            &path,
            bf.map(|f| f.2.as_str()),
            cf.map(|f| f.2.as_str()),
            rf.map(|f| f.2.as_str()),
            raw,
        );
        let pick = choice(&analysis, choices.remove(&path))?;
        if pick == AndroidMergeResolutionChoice::Delete {
            continue;
        }
        let (source, file) = match pick {
            AndroidMergeResolutionChoice::Current => (&c, cf.ok_or(Error::Conflict)?),
            AndroidMergeResolutionChoice::Result => (&r, rf.ok_or(Error::Conflict)?),
            AndroidMergeResolutionChoice::AutoMerged => (&r, rf.ok_or(Error::Conflict)?),
            _ => return Err(Error::Conflict),
        };
        let merged = if pick == AndroidMergeResolutionChoice::AutoMerged {
            Some(analysis.merged.ok_or(Error::Conflict)?)
        } else {
            None
        };
        let sid = Uuid::new_v4();
        let size = merged.as_ref().map(|v| v.len() as u64).unwrap_or(file.1);
        let sha = merged
            .as_ref()
            .map(|v| hash(v))
            .unwrap_or_else(|| file.2.clone());
        file_bytes = file_bytes.checked_add(size).ok_or(Error::BudgetExceeded)?;
        if size > TEAM_BLOB_MAX_BYTES || file_bytes > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
        if let Some(bytes) = merged {
            for (i, page) in bytes.chunks(content::PAGE).enumerate() {
                content::write_page(tx, keys, key, &dest, &format!("file:{sid}"), i as i64, page)
                    .await?;
            }
        } else {
            for i in 0..size.div_ceil(content::PAGE as u64) {
                let bytes =
                    content::read_page(tx, keys, source, &format!("file:{}", file.0), i as i64)
                        .await?;
                content::write_page(
                    tx,
                    keys,
                    key,
                    &dest,
                    &format!("file:{sid}"),
                    i as i64,
                    &bytes,
                )
                .await?;
            }
        }
        let mut metadata = file.4.clone();
        metadata["original_source_id"] = json!(file.0);
        metadata["size"] = json!(size);
        metadata["digest"] = json!(sha);
        sqlx::query("INSERT INTO android_project_version_files(version_id,path,source_id,size,sha256,media_type,payload) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id.to_string()).bind(&path).bind(sid.to_string()).bind(size as i64).bind(&sha).bind(&file.3).bind(json_string(&metadata)?).execute(&mut **tx).await.map_err(storage_error)?;
        digest.update(json_string(&(&path, sid, size, &sha))?.as_bytes());
    }
    if !choices.is_empty() {
        return Err(device_invalid());
    }
    if sqlx::query("UPDATE android_project_versions SET content_sha256=$1 WHERE id=$2 AND ready=0 AND content_sha256=$3").bind(format!("{:x}",digest.finalize())).bind(id.to_string()).bind(hash(b"")).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
    Ok(id)
}

async fn prepared(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: &ProjectScope,
    version_id: Uuid,
    snapshot: Uuid,
    key: &str,
    request: &str,
    kind: &str,
    backend: &str,
    current: i64,
    merge_base: Option<Uuid>,
) -> Result<copy::CopyPlan> {
    let (_, _, source_sha) = version_custody(tx, p.project, version_id, kind == "merge").await?;
    let id = Uuid::new_v4();
    let mut mappings = Vec::new();
    for ((kind, source_id), sha) in metadata(tx, version_id).await? {
        mappings.push(AndroidForkObjectMapping {
            kind: kind.clone(),
            source_id,
            destination_id: if kind == "project" {
                p.project
            } else {
                Uuid::new_v4()
            },
            original_sha256: sha,
        });
    }
    let mut files = Vec::new();
    for (path, (source_id, size, sha, media_type, _)) in file_catalog(tx, version_id).await? {
        let file_id = if p.team.is_some() {
            Uuid::new_v4()
        } else {
            Uuid::parse_str(&hash(json_string(&(snapshot, &path))?.as_bytes())[..32])
                .map_err(storage_error)?
        };
        mappings.push(AndroidForkObjectMapping {
            kind: "file".into(),
            source_id,
            destination_id: file_id,
            original_sha256: sha.clone(),
        });
        let blob = p.team.map(|team| TeamBlob {
            id: file_id,
            team_id: team,
            project_id: p.project,
            name: path.clone(),
            media_type: media_type.clone(),
            size,
            sha256: sha.clone(),
            version: 1,
            revision: 1,
            state: TeamBlobState::Uploading,
            created_at: current,
            updated_at: current,
        });
        let target = p.team.map(|team| TeamBlobVersion {
            blob_id: file_id,
            version: 1,
            size,
            sha256: sha.clone(),
            object_key: format!("cowork/v1/{team}/{}/{file_id}/1/copy-{id}/{sha}", p.project),
            backend_id: backend.into(),
            state: TeamBlobState::Uploading,
            created_at: current,
        });
        files.push(copy::CopyFile {
            path,
            source_id,
            size,
            sha256: sha,
            media_type,
            destination_id: file_id,
            blob,
            target,
            expected_revision: None,
        });
    }
    let plan = copy::CopyPlan {
        format: "android-project-copy-plan/1".into(),
        id,
        owner: actor.identity_id(),
        key: key.into(),
        kind: kind.into(),
        source_project: p.project,
        source_version: version_id,
        source_content_sha256: source_sha,
        source_project_revision: p.revision,
        target_project: p.project,
        target_team: p.team,
        target_name: p.payload["name"].as_str().unwrap_or_default().into(),
        target_revision: Some(p.revision),
        target_current_version: head(tx, p.project).await?,
        snapshot,
        mappings,
        files,
        request_sha256: request.into(),
        created_at: current,
        target_content_sha256: Some(target_digest(store, tx, actor, p.project).await?),
        merge_base,
    };
    if let Some(team) = p.team {
        let existing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM team_blobs WHERE team_id=$1 AND project_id=$2",
        )
        .bind(team.to_string())
        .bind(p.project.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if existing
            .checked_add(plan.files.len() as i64)
            .is_none_or(|n| n > 2000)
        {
            return Err(Error::BudgetExceeded);
        }
    }
    let encoded = json_string(&plan)?;
    if encoded.len() > copy::MAX_PLAN {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO android_project_copy_plans(id,owner_id,operation_key,source_project_id,source_version_id,target_project_id,request_sha256,payload,payload_sha256,state,created_at) VALUES($1,$2,$3,$4,$5,$4,$6,$7,$8,'prepared',$9)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(key).bind(p.project.to_string()).bind(version_id.to_string()).bind(request).bind(&encoded).bind(hash(encoded.as_bytes())).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    for file in &plan.files {
        if let Some(target) = &file.target {
            let fingerprint = hash(
                json_string(&(
                    id,
                    &plan.request_sha256,
                    &file.path,
                    file.source_id,
                    &file.sha256,
                    target,
                ))?
                .as_bytes(),
            );
            sqlx::query("INSERT INTO android_project_copy_effects(plan_id,path,original_sha256,target,request_sha256,state) VALUES($1,$2,$3,$4,$5,'prepared')").bind(id.to_string()).bind(&file.path).bind(&file.sha256).bind(json_string(target)?).bind(fingerprint).execute(&mut **tx).await.map_err(storage_error)?;
        }
    }
    Ok(plan)
}
async fn prior(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: Uuid,
    key: &str,
    request: &str,
) -> Result<Option<AndroidProjectCopy>> {
    scope(store, tx, actor, p, ContentAction::Read).await?;
    let Some(row)=sqlx::query("SELECT id,request_sha256 FROM android_project_copy_plans WHERE owner_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?else{return Ok(None)};
    if row
        .try_get::<String, _>("request_sha256")
        .map_err(storage_error)?
        != request
    {
        return Err(Error::Conflict);
    }
    let (plan, state, receipt) = copy::load(tx, row_id(&row, "id")?).await?;
    copy::rights(store, tx, actor, &plan, false).await?;
    Ok(Some(AndroidProjectCopy::from_parts(plan, state, receipt)))
}
impl Store {
    pub async fn android_project_apply_prepare(
        &self,
        actor: &Principal,
        project: Uuid,
        id: Uuid,
        input: AndroidApplyProjectVersion,
        key: &str,
        backend: &str,
        active_key: KeyId,
    ) -> Result<AndroidProjectCopy> {
        operation_key(key)?;
        let request = hash(
            json_string(&("android-project-version-apply/1", project, id, &input))?.as_bytes(),
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(p) = prior(self, &mut tx, actor, project, key, &request).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(p);
        }
        crate::server_keys::server_admission(&mut tx, active_key).await?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        if p.revision != input.expected_project_revision
            || head(&mut tx, project).await? != input.expected_current_version_id
        {
            return Err(Error::Conflict);
        }
        let plan = prepared(
            self,
            &mut tx,
            actor,
            &p,
            id,
            Uuid::new_v4(),
            key,
            &request,
            "apply",
            backend,
            current,
            None,
        )
        .await?;
        copy::rights(self, &mut tx, actor, &plan, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidProjectCopy::from_parts(
            plan,
            "prepared".into(),
            None,
        ))
    }
    pub async fn android_project_merge_prepare(
        &self,
        actor: &Principal,
        project: Uuid,
        input: AndroidApplyProjectMerge,
        key: &str,
        backend: &str,
        keys: &dyn KeyStore,
        cipher: KeyId,
    ) -> Result<AndroidProjectCopy> {
        operation_key(key)?;
        let request =
            hash(json_string(&("android-project-version-merge/1", project, &input))?.as_bytes());
        let cipher = crate::android_private_workspace::writer_cipher(keys, cipher)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, cipher).await?;
        let current = device_writer(&mut tx).await?;
        if let Some(p) = prior(self, &mut tx, actor, project, key, &request).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(p);
        }
        let p = scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        if p.revision != input.expected_project_revision
            || head(&mut tx, project)
                .await?
                .is_some_and(|v| v != input.current_version_id)
        {
            return Err(Error::Conflict);
        }
        let snapshot = Uuid::new_v4();
        let merged =
            merge_archive(&mut tx, keys, cipher, actor, &p, &input, snapshot, current).await?;
        let plan = prepared(
            self,
            &mut tx,
            actor,
            &p,
            merged,
            snapshot,
            key,
            &request,
            "merge",
            backend,
            current,
            Some(input.base_version_id),
        )
        .await?;
        copy::rights(self, &mut tx, actor, &plan, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidProjectCopy::from_parts(
            plan,
            "prepared".into(),
            None,
        ))
    }
    pub async fn android_project_merge_review(
        &self,
        actor: &Principal,
        project: Uuid,
        base: Uuid,
        current: Uuid,
        result: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<AndroidProjectMergeReview> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let (_, b, _) = version(&mut tx, project, base).await?;
        let (_, c, _) = version(&mut tx, project, current).await?;
        let (_, r, _) = version(&mut tx, project, result).await?;
        let bm = metadata(&mut tx, base).await?;
        let cm = metadata(&mut tx, current).await?;
        let rm = metadata(&mut tx, result).await?;
        let mut reviews = Vec::new();
        for (kind, id) in bm
            .keys()
            .chain(cm.keys())
            .chain(rm.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            let a = merge::analyze(
                &format!("@content/{kind}/{id}"),
                bm.get(&(kind.clone(), id)).map(String::as_str),
                cm.get(&(kind.clone(), id)).map(String::as_str),
                rm.get(&(kind, id)).map(String::as_str),
                None,
            );
            reviews.push(a.review);
        }
        let bf = file_catalog(&mut tx, base).await?;
        let cf = file_catalog(&mut tx, current).await?;
        let rf = file_catalog(&mut tx, result).await?;
        for path in bf
            .keys()
            .chain(cf.keys())
            .chain(rf.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            let bv = bf.get(&path);
            let cv = cf.get(&path);
            let rv = rf.get(&path);
            let bt = file_text(&mut tx, keys, &b, bv).await?;
            let ct = file_text(&mut tx, keys, &c, cv).await?;
            let rt = file_text(&mut tx, keys, &r, rv).await?;
            let raw = bt
                .as_deref()
                .zip(ct.as_deref())
                .zip(rt.as_deref())
                .map(|((b, c), r)| (b, c, r));
            reviews.push(
                merge::analyze(
                    &path,
                    bv.map(|v| v.2.as_str()),
                    cv.map(|v| v.2.as_str()),
                    rv.map(|v| v.2.as_str()),
                    raw,
                )
                .review,
            );
        }
        let value = AndroidProjectMergeReview {
            schema_version: 2,
            project_id: project,
            base_version_id: base,
            current_version_id: current,
            result_version_id: result,
            files: reviews,
        };
        if json_string(&value)?.len() > 3 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_project_copy_commit(
        &self,
        actor: &Principal,
        id: Uuid,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<AndroidProjectVersion> {
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let (p, state, old) = copy::load(&mut tx, id).await?;
        if !matches!(p.kind.as_str(), "apply" | "merge") {
            return Err(Error::Conflict);
        }
        copy::rights(self, &mut tx, actor, &p, false).await?;
        if let Some(old) = old {
            if state != "committed" {
                return Err(Error::Conflict);
            }
            let v = serde_json::from_value(old).map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if state == "retired" {
            return Err(Error::Conflict);
        }
        copy::rights(self, &mut tx, actor, &p, true).await?;
        let actual = scope(self, &mut tx, actor, p.target_project, ContentAction::Write).await?;
        if Some(actual.revision) != p.target_revision
            || head(&mut tx, p.target_project).await? != p.target_current_version
            || Some(target_digest(self, &mut tx, actor, p.target_project).await?)
                != p.target_content_sha256
        {
            return Err(Error::Conflict);
        }
        // Current rows become inert history. Original versions, runs, grants and external objects survive.
        if p.target_team.is_none() {
            sqlx::query(
                "UPDATE android_personal_threads SET deleted=1 WHERE project_id=$1 AND deleted=0",
            )
            .bind(p.target_project.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
            sqlx::query(
                "UPDATE android_personal_messages SET deleted=1 WHERE project_id=$1 AND deleted=0",
            )
            .bind(p.target_project.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
            sqlx::query(
                "UPDATE android_private_files SET deleted=1 WHERE project_id=$1 AND deleted=0",
            )
            .bind(p.target_project.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        } else {
            let rows: Vec<String> =
                sqlx::query_scalar("SELECT payload FROM team_orders WHERE project_id=$1")
                    .bind(p.target_project.to_string())
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            for raw in rows {
                let mut order: TeamOrder = parse(&raw)?;
                order.archived = true;
                order.revision = order.revision.checked_add(1).ok_or(Error::Conflict)?;
                sqlx::query("UPDATE team_orders SET payload=$1 WHERE id=$2 AND project_id=$3 AND payload=$4").bind(json_string(&order)?).bind(order.id.to_string()).bind(p.target_project.to_string()).bind(raw).execute(&mut *tx).await.map_err(storage_error)?;
            }
            sqlx::query("DELETE FROM team_documents WHERE project_id=$1")
                .bind(p.target_project.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            // Metadata deletion retains all original blob versions and does not send DELETE to S3.
            let blobs: Vec<String> =
                sqlx::query_scalar("SELECT payload FROM team_blobs WHERE project_id=$1")
                    .bind(p.target_project.to_string())
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            for raw in blobs {
                let mut blob: TeamBlob = parse(&raw)?;
                blob.state = TeamBlobState::Deleted;
                blob.revision = blob.revision.checked_add(1).ok_or(Error::Conflict)?;
                sqlx::query("UPDATE team_blobs SET revision=$1,payload=$2 WHERE id=$3 AND project_id=$4 AND payload=$5").bind(blob.revision).bind(json_string(&blob)?).bind(blob.id.to_string()).bind(p.target_project.to_string()).bind(raw).execute(&mut *tx).await.map_err(storage_error)?;
            }
        }
        sqlx::query("UPDATE android_task_versions SET deleted=1,released=0 WHERE project_id=$1")
            .bind(p.target_project.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        super::materialize::objects(self, &mut tx, actor, &p, keys, key, current).await?;
        super::materialize::files(&mut tx, actor, &p, keys, key, current).await?;
        let next = actual.revision.checked_add(1).ok_or(Error::Conflict)?;
        if p.target_team.is_some() {
            if sqlx::query("INSERT INTO team_project_profiles(project_id,revision,name,archived) VALUES($1,$2,$3,0) ON CONFLICT(project_id) DO UPDATE SET revision=excluded.revision WHERE team_project_profiles.revision=$4 AND team_project_profiles.archived=0").bind(p.target_project.to_string()).bind(next).bind(&p.target_name).bind(actual.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
            sqlx::query("UPDATE identity_resources SET revision=revision+1 WHERE id=$1")
                .bind(p.target_project.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
        } else {
            let mut payload = actual.payload.clone();
            payload["current_version_id"] = json!(p.source_version);
            payload["revision"] = json!(next);
            payload["etag"] = json!(format!("{}:{next}", p.target_project));
            payload["updated_at"] = json!(device_iso(current)?);
            if sqlx::query("UPDATE android_personal_projects SET revision=$1,payload=$2 WHERE id=$3 AND revision=$4 AND deleted=0").bind(next).bind(json_string(&payload)?).bind(p.target_project.to_string()).bind(actual.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        }
        sqlx::query("INSERT INTO android_project_version_heads(project_id,team_id,current_version_id,active_snapshot_id,revision) VALUES($1,$2,$3,$4,$5) ON CONFLICT(project_id) DO UPDATE SET current_version_id=excluded.current_version_id,active_snapshot_id=excluded.active_snapshot_id,revision=excluded.revision").bind(p.target_project.to_string()).bind(p.target_team.map(|v|v.to_string())).bind(p.source_version.to_string()).bind(p.snapshot.to_string()).bind(next).execute(&mut *tx).await.map_err(storage_error)?;
        if p.kind == "merge" {
            if sqlx::query("UPDATE android_project_versions SET ready=1 WHERE id=$1 AND ready=0 AND creator_id=$2 AND content_sha256=$3").bind(p.source_version.to_string()).bind(p.owner.to_string()).bind(&p.source_content_sha256).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        }
        let (value, _, _) = version(&mut tx, p.target_project, p.source_version).await?;
        let encoded = json_string(&value)?;
        if sqlx::query("UPDATE android_project_copy_plans SET state='committed',receipt=$1,receipt_sha256=$2 WHERE id=$3 AND state IN('prepared','receiving','uncertain') AND receipt IS NULL AND payload_sha256=$4").bind(&encoded).bind(hash(encoded.as_bytes())).bind(p.id.to_string()).bind(hash(json_string(&p)?.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        receipt(
            &mut tx,
            actor.identity_id(),
            p.target_project,
            &p.key,
            &p.kind,
            &p.request_sha256,
            &serde_json::to_value(&value).map_err(storage_error)?,
        )
        .await?;
        let current_project =
            scope(self, &mut tx, actor, p.target_project, ContentAction::Write).await?;
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "project",
            p.target_project,
            p.target_team,
            Some(p.target_project),
            next,
            &current_project.payload,
            false,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
