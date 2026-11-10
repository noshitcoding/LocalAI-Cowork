//! V1 team snapshot protocol backed by authenticated bounded pages, with actual team rights.
use super::*;
use zeroize::Zeroizing;

pub(super) async fn required_keys(tx: &mut Transaction<'_, Any>) -> Result<Vec<KeyId>> {
    let mut found = BTreeMap::new();
    let mut after = (String::new(), String::new(), -1i64);
    loop {
        let row = sqlx::query("SELECT c.snapshot_id,c.digest,c.ordinal,c.page_digest,c.size,LENGTH(c.sealed) AS sealed_length,CASE WHEN LENGTH(c.sealed)<=2097152 THEN c.sealed ELSE NULL END AS sealed,s.creator_id,s.project_id FROM android_project_snapshot_chunks c LEFT JOIN android_project_snapshots s ON s.id=c.snapshot_id WHERE c.snapshot_id>$1 OR (c.snapshot_id=$1 AND c.digest>$2) OR (c.snapshot_id=$1 AND c.digest=$2 AND c.ordinal>$3) ORDER BY c.snapshot_id,c.digest,c.ordinal LIMIT 1").bind(&after.0).bind(&after.1).bind(after.2).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let Some(row) = row else { break };
        let snapshot: String = row.try_get("snapshot_id").map_err(storage_error)?;
        let digest: String = row.try_get("digest").map_err(storage_error)?;
        let ordinal: i64 = row.try_get("ordinal").map_err(storage_error)?;
        Uuid::parse_str(&snapshot).map_err(|_| Error::Forbidden)?;
        row_id(&row, "creator_id")?;
        row_id(&row, "project_id")?;
        if !(0..4).contains(&ordinal)
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || row
                .try_get::<i64, _>("sealed_length")
                .map_err(storage_error)?
                > (2 * content::PAGE) as i64
        {
            return Err(Error::Forbidden);
        }
        let key = content::envelope_key(
            &row.try_get::<Option<String>, _>("sealed")
                .map_err(storage_error)?
                .ok_or(Error::Forbidden)?,
            row.try_get("size").map_err(storage_error)?,
            &row.try_get::<String, _>("page_digest")
                .map_err(storage_error)?,
        )?;
        found.insert((key.instance, key.key), key);
        after = (snapshot, digest, ordinal);
    }
    Ok(found.into_values().collect())
}

pub(super) fn declared_chunks(value: &Value) -> Result<BTreeMap<String, usize>> {
    if value.get("expires_at").is_some_and(|v| {
        !v.is_null()
            && v.as_str()
                .is_none_or(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
    }) {
        return Err(device_invalid());
    }
    let files = value["files"]
        .as_array()
        .filter(|v| v.len() <= 2000)
        .ok_or_else(device_invalid)?;
    let mut paths = BTreeSet::new();
    let mut chunks = BTreeMap::new();
    let mut total = 0u64;
    for file in files {
        let path = file["path"].as_str().ok_or_else(device_invalid)?;
        crate::android_private_snapshots::relative(path)?;
        if !paths.insert(path.to_lowercase()) {
            return Err(Error::Conflict);
        }
        let size = file["size"]
            .as_u64()
            .filter(|n| *n <= TEAM_BLOB_MAX_BYTES)
            .ok_or(Error::BudgetExceeded)?;
        if file.get("mode").is_some_and(|v| {
            v.as_u64()
                .is_none_or(|n| n > u32::MAX as u64 || !matches!(n & 0o170000, 0 | 0o100000))
        }) {
            return Err(device_invalid());
        }
        if file.get("modified_at").is_some_and(|v| {
            v.as_str()
                .is_none_or(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
        }) {
            return Err(device_invalid());
        }
        let parts = file["chunks"]
            .as_array()
            .filter(|v| v.len() <= 4096)
            .ok_or_else(device_invalid)?;
        let mut file_size = 0u64;
        for part in parts {
            let d = part["digest"]
                .as_str()
                .filter(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                .ok_or_else(device_invalid)?;
            let n = part["plaintext_size"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 4 * 1024 * 1024)
                .ok_or_else(device_invalid)?;
            if chunks
                .insert(d.into(), n as usize)
                .is_some_and(|old| old != n as usize)
            {
                return Err(Error::Conflict);
            }
            file_size = file_size.checked_add(n).ok_or(Error::BudgetExceeded)?;
        }
        if file_size != size {
            return Err(Error::Conflict);
        }
        total = total.checked_add(size).ok_or(Error::BudgetExceeded)?;
        if total > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
    }
    if value["total_bytes"].as_u64() != Some(total) {
        return Err(Error::Conflict);
    }
    Ok(chunks)
}

pub(super) async fn rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let mut after = (String::new(), String::new(), -1i64);
    let mut count = 0u64;
    loop {
        let r=sqlx::query("SELECT c.snapshot_id,c.digest,c.ordinal,c.page_digest,c.size,c.sealed,s.creator_id,s.project_id FROM android_project_snapshot_chunks c JOIN android_project_snapshots s ON s.id=c.snapshot_id WHERE c.snapshot_id>$1 OR (c.snapshot_id=$1 AND c.digest>$2) OR (c.snapshot_id=$1 AND c.digest=$2 AND c.ordinal>$3) ORDER BY c.snapshot_id,c.digest,c.ordinal LIMIT 1").bind(&after.0).bind(&after.1).bind(after.2).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let Some(r) = r else { break };
        let snapshot: String = r.try_get("snapshot_id").map_err(storage_error)?;
        let digest: String = r.try_get("digest").map_err(storage_error)?;
        let ordinal: i64 = r.try_get("ordinal").map_err(storage_error)?;
        after = (snapshot.clone(), digest.clone(), ordinal);
        let original: String = r.try_get("sealed").map_err(storage_error)?;
        let value: Value = parse(&original)?;
        let key: KeyId = serde_json::from_value(value["key_id"].clone())
            .map_err(|_| Error::NativeKeyUnavailable)?;
        if key.instance != target.instance {
            return Err(Error::NativeKeyUnavailable);
        }
        let s = content::Scope {
            owner: row_id(&r, "creator_id")?,
            project: row_id(&r, "project_id")?,
            version: Uuid::parse_str(&snapshot).map_err(storage_error)?,
        };
        let page_digest: String = r.try_get("page_digest").map_err(storage_error)?;
        let size: i64 = r.try_get("size").map_err(storage_error)?;
        let bytes = content::open(
            keys,
            &s,
            &format!("source:{digest}"),
            ordinal,
            &page_digest,
            usize::try_from(size).map_err(|_| Error::Forbidden)?,
            &value,
        )?;
        if key == target {
            continue;
        }
        let sealed = content::seal(
            keys,
            target,
            &s,
            &format!("source:{digest}"),
            ordinal,
            &bytes,
        )?;
        let changed=sqlx::query("UPDATE android_project_snapshot_chunks SET sealed=$1 WHERE snapshot_id=$2 AND digest=$3 AND ordinal=$4 AND page_digest=$5 AND size=$6 AND sealed=$7").bind(json_string(&sealed)?).bind(snapshot).bind(digest).bind(ordinal).bind(page_digest).bind(size).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        count = count.checked_add(1).ok_or(Error::BudgetExceeded)?;
    }
    Ok(count)
}
pub(super) async fn row(tx: &mut Transaction<'_, Any>, id: Uuid) -> Result<Option<AnyRow>> {
    sqlx::query("SELECT * FROM android_project_snapshots WHERE id=$1")
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)
}
pub(super) fn snapshot_scope(row: &AnyRow) -> Result<content::Scope> {
    Ok(content::Scope {
        owner: row_id(row, "creator_id")?,
        project: row_id(row, "project_id")?,
        version: row_id(row, "id")?,
    })
}
pub(super) fn manifest(row: &AnyRow, current: i64) -> Result<Value> {
    let value: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if value["id"] != json!(row_id(row, "id")?)
        || value["project_id"] != json!(row_id(row, "project_id")?)
    {
        return Err(Error::Conflict);
    }
    if value["expires_at"]
        .as_str()
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.timestamp_millis())
                .map_err(storage_error)
        })
        .transpose()?
        .is_some_and(|t| t <= current)
    {
        return Err(Error::Forbidden);
    }
    declared_chunks(&value)?;
    Ok(value)
}
pub(super) async fn read_chunk(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    s: &content::Scope,
    digest: &str,
    size: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    if size == 0 || size > 4 * 1024 * 1024 {
        return Err(Error::Forbidden);
    }
    let mut out = Zeroizing::new(Vec::with_capacity(size));
    for ordinal in 0..size.div_ceil(content::PAGE) {
        let r=sqlx::query("SELECT page_digest,size,sealed FROM android_project_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2 AND ordinal=$3").bind(s.version.to_string()).bind(digest).bind(ordinal as i64).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let n = usize::try_from(r.try_get::<i64, _>("size").map_err(storage_error)?)
            .map_err(|_| Error::Forbidden)?;
        let bytes = content::open(
            keys,
            s,
            &format!("source:{digest}"),
            ordinal as i64,
            &r.try_get::<String, _>("page_digest")
                .map_err(storage_error)?,
            n,
            &parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?,
        )?;
        if out.len() + bytes.len() > size {
            return Err(Error::Forbidden);
        }
        out.extend_from_slice(&bytes);
    }
    if out.len() != size || hash(&out) != digest {
        return Err(Error::Conflict);
    }
    Ok(out)
}
async fn status(tx: &mut Transaction<'_, Any>, row: &AnyRow, current: i64) -> Result<Value> {
    let value = manifest(row, current)?;
    let mut missing = Vec::new();
    for (d, size) in declared_chunks(&value)? {
        let bytes:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM android_project_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2").bind(row_id(row,"id")?.to_string()).bind(&d).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if bytes != size as i64 {
            missing.push(d);
        }
    }
    Ok(
        json!({"schema_version":2,"manifest_id":row_id(row,"id")?,"missing_chunks":missing,"max_chunk_bytes":4*1024*1024,"expires_at":value["expires_at"],"warnings":[]}),
    )
}
impl Store {
    pub async fn android_project_snapshot_list(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let sql = if p.team.is_some() {
            "SELECT id,payload FROM android_project_snapshots WHERE project_id=$1 AND committed=1 AND id>$2 AND CAST($3 AS TEXT)=CAST($3 AS TEXT) ORDER BY id LIMIT 1"
        } else {
            "SELECT id,payload FROM android_private_snapshots WHERE project_id=$1 AND owner_id=$3 AND committed=1 AND deleted=0 AND id>$2 ORDER BY id LIMIT 1"
        };
        let mut after = String::new();
        let mut out = Vec::new();
        loop {
            let Some(row) = sqlx::query(sql)
                .bind(project.to_string())
                .bind(&after)
                .bind(actor.identity_id().to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
            else {
                break;
            };
            after = row.try_get("id").map_err(storage_error)?;
            let raw: String = row.try_get("payload").map_err(storage_error)?;
            if raw.len() > 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            let manifest: Value = parse(&raw)?;
            if manifest["project_id"] != json!(project) || manifest["id"] != after {
                return Err(Error::Conflict);
            }
            if manifest["expires_at"]
                .as_str()
                .map(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map(|v| v.timestamp_millis())
                        .map_err(storage_error)
                })
                .transpose()?
                .is_some_and(|t| t <= current)
            {
                continue;
            }
            if out.len() >= 1000 {
                return Err(Error::BudgetExceeded);
            }
            out.push(json!({"id":after,"project_id":project,"created_at":manifest["created_at"],"total_bytes":manifest["total_bytes"],"file_count":manifest["files"].as_array().ok_or(Error::Conflict)?.len()}));
        }
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(out)
    }
    /// None means the actual project is private, whose existing snapshot authority is retained.
    pub async fn android_project_snapshot_begin(
        &self,
        actor: &Principal,
        mut body: Value,
        key: Option<&str>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Option<Value>> {
        if body
            .as_object()
            .ok_or_else(device_invalid)?
            .keys()
            .any(|k| !["project_id", "total_bytes", "files", "expires_at"].contains(&k.as_str()))
        {
            return Err(device_invalid());
        }
        if json_string(&body)?.len() > 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let project = crate::android_personal::wire_uuid(&body, "project_id")?;
        declared_chunks(&body)?;
        let key_id = crate::android_private_workspace::writer_cipher(keys, key_id)?;
        let request = hash(json_string(&body)?.as_bytes());
        let operation = key
            .map(str::to_owned)
            .unwrap_or_else(|| format!("snapshot:{request}"));
        operation_key(&operation)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        let current = device_writer(&mut tx).await?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        let Some(team) = p.team else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        if let Some(r) = sqlx::query(
            "SELECT * FROM android_project_snapshots WHERE creator_id=$1 AND creation_key=$2",
        )
        .bind(actor.identity_id().to_string())
        .bind(&operation)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if row_id(&r, "project_id")? != project
                || r.try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != request
            {
                return Err(Error::Conflict);
            }
            let value = status(&mut tx, &r, current).await?;
            scope(self, &mut tx, actor, project, ContentAction::Write).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(value));
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_project_snapshots WHERE project_id=$1",
        )
        .bind(project.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 1000 {
            return Err(Error::BudgetExceeded);
        }
        let id = Uuid::new_v4();
        body["schema_version"] = json!(2);
        body["id"] = json!(id);
        body["encryption_key_id"] = json!(key_id
            .cipher_id()
            .map_err(|_| Error::NativeKeyUnavailable)?);
        body["created_at"] = json!(device_iso(current)?);
        for file in body["files"].as_array_mut().ok_or_else(device_invalid)? {
            for chunk in file["chunks"].as_array_mut().ok_or_else(device_invalid)? {
                let n = chunk["plaintext_size"]
                    .as_u64()
                    .ok_or_else(device_invalid)?;
                chunk["ciphertext_size"] = json!(n + n.div_ceil(content::PAGE as u64) * 16);
            }
        }
        sqlx::query("INSERT INTO android_project_snapshots(id,project_id,team_id,creator_id,creation_key,request_sha256,payload,committed) VALUES($1,$2,$3,$4,$5,$6,$7,0)").bind(id.to_string()).bind(project.to_string()).bind(team.to_string()).bind(actor.identity_id().to_string()).bind(operation).bind(request).bind(json_string(&body)?).execute(&mut *tx).await.map_err(storage_error)?;
        let r = row(&mut tx, id).await?.ok_or(Error::Conflict)?;
        let value = status(&mut tx, &r, current).await?;
        scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(value))
    }
    pub async fn android_project_snapshot(
        &self,
        actor: &Principal,
        id: Uuid,
        upload_status: bool,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let Some(r) = row(&mut tx, id).await? else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        let project = row_id(&r, "project_id")?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let value = if upload_status {
            status(&mut tx, &r, current).await?
        } else {
            manifest(&r, current)?
        };
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(value))
    }
    pub async fn android_project_snapshot_chunk(
        &self,
        actor: &Principal,
        id: Uuid,
        digest: &str,
        supplied: Option<&[u8]>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Option<(Value, Zeroizing<Vec<u8>>)>> {
        let key_id = if supplied.is_some() {
            crate::android_private_workspace::writer_cipher(keys, key_id)?
        } else {
            key_id
        };
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if supplied.is_some() {
            crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        }
        let current = device_writer(&mut tx).await?;
        let Some(r) = row(&mut tx, id).await? else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        let s = snapshot_scope(&r)?;
        let action = if supplied.is_some() {
            ContentAction::Write
        } else {
            ContentAction::Read
        };
        scope(self, &mut tx, actor, s.project, action).await?;
        let value = manifest(&r, current)?;
        let size = *declared_chunks(&value)?
            .get(digest)
            .ok_or(Error::NotFound)?;
        let exists:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_project_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2").bind(id.to_string()).bind(digest).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let result = if let Some(bytes) = supplied {
            if s.owner != actor.identity_id() || bytes.len() != size || hash(bytes) != digest {
                return Err(Error::Forbidden);
            }
            if exists != 0 {
                let actual = read_chunk(&mut tx, keys, &s, digest, size).await?;
                if actual.as_slice() != bytes {
                    return Err(Error::Conflict);
                }
            } else {
                if r.try_get::<i64, _>("committed").map_err(storage_error)? != 0 {
                    return Err(Error::Conflict);
                }
                for (ordinal, page) in bytes.chunks(content::PAGE).enumerate() {
                    let sealed = content::seal(
                        keys,
                        key_id,
                        &s,
                        &format!("source:{digest}"),
                        ordinal as i64,
                        page,
                    )?;
                    sqlx::query("INSERT INTO android_project_snapshot_chunks(snapshot_id,digest,ordinal,page_digest,size,sealed) VALUES($1,$2,$3,$4,$5,$6)").bind(id.to_string()).bind(digest).bind(ordinal as i64).bind(hash(page)).bind(page.len() as i64).bind(json_string(&sealed)?).execute(&mut *tx).await.map_err(storage_error)?;
                }
            }
            Zeroizing::new(Vec::new())
        } else {
            read_chunk(&mut tx, keys, &s, digest, size).await?
        };
        let receipt = json!({"schema_version":2,"manifest_id":id,"digest":digest,"plaintext_size":size,"ciphertext_size":size+size.div_ceil(content::PAGE)*16,"deduplicated":exists!=0,"warnings":[]});
        scope(self, &mut tx, actor, s.project, action).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some((receipt, result)))
    }
    pub async fn android_project_snapshot_commit(
        &self,
        actor: &Principal,
        id: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let Some(r) = row(&mut tx, id).await? else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        let s = snapshot_scope(&r)?;
        scope(self, &mut tx, actor, s.project, ContentAction::Write).await?;
        if s.owner != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        let mut value = manifest(&r, current)?;
        for file in value["files"].as_array_mut().ok_or_else(device_invalid)? {
            let mut h = Sha256::new();
            for chunk in file["chunks"].as_array().ok_or_else(device_invalid)? {
                let d = chunk["digest"].as_str().ok_or_else(device_invalid)?;
                let n = usize::try_from(
                    chunk["plaintext_size"]
                        .as_u64()
                        .ok_or_else(device_invalid)?,
                )
                .map_err(|_| Error::BudgetExceeded)?;
                let bytes = read_chunk(&mut tx, keys, &s, d, n).await?;
                h.update(bytes.as_slice());
            }
            let digest = format!("{:x}", h.finalize());
            if file
                .get("digest")
                .is_some_and(|v| v.as_str() != Some(digest.as_str()))
            {
                return Err(Error::Conflict);
            }
            file["digest"] = json!(digest);
        }
        if r.try_get::<i64, _>("committed").map_err(storage_error)? == 0 {
            let changed=sqlx::query("UPDATE android_project_snapshots SET committed=1,payload=$1 WHERE id=$2 AND committed=0 AND payload=$3").bind(json_string(&value)?).bind(id.to_string()).bind(r.try_get::<String,_>("payload").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
            if changed != 1 {
                return Err(Error::Conflict);
            }
        }
        scope(self, &mut tx, actor, s.project, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(value))
    }
}
