//! Privileged rotation-only codecs. The caller holds the admitted server-key SQL transaction.
//! All business bytes, revisions, original operation identities and authentication keys survive unchanged.
use super::*;
use crate::android_private_server::{id, row};
use crate::android_private_workspace as workspace;
use cowork_mcp::{CheckpointScope, SealedSession, SessionCipher};
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use sqlx::{Any, Transaction};
const ROW_LIMIT: usize = 100_000;
fn owned(value: &Value, target: KeyId) -> Result<bool> {
    let original: KeyId =
        serde_json::from_value(value["key_id"].clone()).map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(original.instance == target.instance && original != target)
}
fn bounded(n: usize) -> Result<()> {
    if n > ROW_LIMIT {
        Err(Error::BudgetExceeded)
    } else {
        Ok(())
    }
}
fn one(n: u64) -> Result<()> {
    if n == 1 { Ok(()) } else { Err(Error::Conflict) }
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    target.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    keys.load(&target)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut count = 0u64;
    let providers=sqlx::query("SELECT profile_id,profile_revision,scope,sealed FROM android_personal_provider_secrets ORDER BY profile_id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    bounded(providers.len())?;
    for p in providers {
        let original: String = p.try_get("sealed").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance || sealed.key_id() == target {
            continue;
        }
        let scope: CredentialScope =
            parse(&p.try_get::<String, _>("scope").map_err(storage_error)?)?;
        if scope.server_id
            != p.try_get::<String, _>("profile_id")
                .map_err(storage_error)?
            || scope.revision
                != p.try_get::<i64, _>("profile_revision")
                    .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let next =
            seal_bearer(keys, target, &scope, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        one(sqlx::query("UPDATE android_personal_provider_secrets SET sealed=$1 WHERE profile_id=$2 AND profile_revision=$3 AND sealed=$4").bind(json_string(&next)?).bind(&scope.server_id).bind(scope.revision).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    let snapshots=sqlx::query("SELECT c.snapshot_id,c.digest,c.size,c.sealed,s.owner_id,s.project_id FROM android_private_snapshot_chunks c JOIN android_private_snapshots s ON s.id=c.snapshot_id ORDER BY c.snapshot_id,c.digest LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    bounded(snapshots.len())?;
    for s in snapshots {
        let original: String = s.try_get("sealed").map_err(storage_error)?;
        let sealed: Value = parse(&original)?;
        if !owned(&sealed, target)? {
            continue;
        }
        let owner = id(&s, "owner_id")?;
        let project = id(&s, "project_id")?;
        let snapshot = id(&s, "snapshot_id")?;
        let digest: String = s.try_get("digest").map_err(storage_error)?;
        let size: i64 = s.try_get("size").map_err(storage_error)?;
        let plain = crate::android_private_snapshots::open(
            keys,
            owner,
            project,
            snapshot,
            &digest,
            usize::try_from(size).map_err(|_| Error::Forbidden)?,
            &sealed,
        )?;
        let next = crate::android_private_snapshots::seal(
            keys, target, owner, project, snapshot, &digest, &plain,
        )?;
        one(sqlx::query("UPDATE android_private_snapshot_chunks SET sealed=$1 WHERE snapshot_id=$2 AND digest=$3 AND size=$4 AND sealed=$5").bind(json_string(&next)?).bind(snapshot.to_string()).bind(digest).bind(size).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    let bindings = sqlx::query(
        "SELECT * FROM android_private_mcp_bindings ORDER BY owner_id,project_id,entity_id LIMIT 100001",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    bounded(bindings.len())?;
    for b in bindings {
        let original: String = b.try_get("sealed").map_err(storage_error)?;
        let sealed: Value = parse(&original)?;
        if !owned(&sealed, target)? {
            continue;
        }
        let owner = id(&b, "owner_id")?;
        let project = id(&b, "project_id")?;
        let entity = id(&b, "entity_id")?;
        let next = crate::android_private_mcp::rewrap_private_mcp_binding(
            keys, target, owner, project, entity, &b,
        )?;
        one(sqlx::query("UPDATE android_private_mcp_bindings SET sealed=$1 WHERE owner_id=$2 AND project_id=$3 AND entity_id=$4 AND revision=$5 AND binding_sha=$6 AND sealed=$7").bind(json_string(&next)?).bind(owner.to_string()).bind(project.to_string()).bind(entity.to_string()).bind(b.try_get::<i64,_>("revision").map_err(storage_error)?).bind(b.try_get::<String,_>("binding_sha").map_err(storage_error)?).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    let checkpoints = sqlx::query(
        "SELECT run_id,digest,sealed FROM android_private_loop_checkpoints ORDER BY run_id LIMIT 100001",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    bounded(checkpoints.len())?;
    for c in checkpoints {
        let original: String = c.try_get("sealed").map_err(storage_error)?;
        let sealed: Value = parse(&original)?;
        if !owned(&sealed, target)? {
            continue;
        }
        let run = id(&c, "run_id")?;
        let r = row(tx, run).await?;
        let plain = workspace::open_value(keys, &r, "checkpoint", &sealed)?;
        let value: Value = serde_json::from_slice(&plain).map_err(storage_error)?;
        if canonical_hash(&value) != c.try_get::<String, _>("digest").map_err(storage_error)? {
            return Err(Error::Forbidden);
        }
        let next = workspace::seal_value(keys, target, &r, "checkpoint", &plain)?;
        one(sqlx::query("UPDATE android_private_loop_checkpoints SET sealed=$1 WHERE run_id=$2 AND digest=$3 AND sealed=$4").bind(json_string(&next)?).bind(run.to_string()).bind(c.try_get::<String,_>("digest").map_err(storage_error)?).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    let steps=sqlx::query("SELECT run_id,sequence,intent,request_sha,receipt,receipt_sha FROM android_private_loop_steps ORDER BY run_id,sequence LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    bounded(steps.len())?;
    for s in steps {
        let run = id(&s, "run_id")?;
        let r = row(tx, run).await?;
        let sequence: i64 = s.try_get("sequence").map_err(storage_error)?;
        for (field, digest_field) in [("intent", "request_sha"), ("receipt", "receipt_sha")] {
            let Some(original) = s
                .try_get::<Option<String>, _>(field)
                .map_err(storage_error)?
            else {
                continue;
            };
            let sealed: Value = parse(&original)?;
            if !owned(&sealed, target)? {
                continue;
            }
            let locator = format!("{field}:{sequence}");
            let plain = workspace::open_value(keys, &r, &locator, &sealed)?;
            let value: Value = serde_json::from_slice(&plain).map_err(storage_error)?;
            if Some(canonical_hash(&value))
                != s.try_get::<Option<String>, _>(digest_field)
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let next = workspace::seal_value(keys, target, &r, &locator, &plain)?;
            let sql = if field == "intent" {
                "UPDATE android_private_loop_steps SET intent=$1 WHERE run_id=$2 AND sequence=$3 AND intent=$4"
            } else {
                "UPDATE android_private_loop_steps SET receipt=$1 WHERE run_id=$2 AND sequence=$3 AND receipt=$4"
            };
            one(sqlx::query(sql)
                .bind(json_string(&next)?)
                .bind(run.to_string())
                .bind(sequence)
                .bind(original)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?
                .rows_affected())?;
            count += 1;
        }
    }
    let versions = sqlx::query(
        "SELECT id,run_id,sealed FROM android_private_loop_versions ORDER BY run_id,id LIMIT 100001",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    bounded(versions.len())?;
    for v in versions {
        let original: String = v.try_get("sealed").map_err(storage_error)?;
        let sealed: Value = parse(&original)?;
        if !owned(&sealed, target)? {
            continue;
        }
        let run = id(&v, "run_id")?;
        let r = row(tx, run).await?;
        let version: String = v.try_get("id").map_err(storage_error)?;
        let plain = workspace::version_bytes(tx, &r, &version, keys).await?;
        let next =
            workspace::seal_value(keys, target, &r, &format!("file-version:{version}"), &plain)?;
        one(sqlx::query("UPDATE android_private_loop_versions SET sealed=$1 WHERE id=$2 AND run_id=$3 AND sealed=$4").bind(json_string(&next)?).bind(version).bind(run.to_string()).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    let sessions=sqlx::query("SELECT run_id,entity_id,binding_sha,catalog_sha,definitions,checkpoint,revision FROM android_private_mcp_sessions ORDER BY run_id,entity_id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    bounded(sessions.len())?;
    for s in sessions {
        let run = id(&s, "run_id")?;
        let entity = id(&s, "entity_id")?;
        let r = row(tx, run).await?;
        let definitions: String = s.try_get("definitions").map_err(storage_error)?;
        let sealed: Value = parse(&definitions)?;
        if sealed != json!({}) && owned(&sealed, target)? {
            let locator = format!("mcp-catalog:{entity}");
            let plain = workspace::open_value(keys, &r, &locator, &sealed)?;
            let values: Vec<Value> = serde_json::from_slice(&plain).map_err(storage_error)?;
            if cowork_mcp::Catalog::from_definitions(values)
                .map_err(|_| Error::Forbidden)?
                .sha256()
                != s.try_get::<String, _>("catalog_sha")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let next = workspace::seal_value(keys, target, &r, &locator, &plain)?;
            one(sqlx::query("UPDATE android_private_mcp_sessions SET definitions=$1 WHERE run_id=$2 AND entity_id=$3 AND definitions=$4").bind(json_string(&next)?).bind(run.to_string()).bind(entity.to_string()).bind(definitions).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
            count += 1;
        }
        if let Some(original) = s
            .try_get::<Option<String>, _>("checkpoint")
            .map_err(storage_error)?
        {
            let mut wrapper: Value = parse(&original)?;
            if wrapper["transport"] != "streamable_http" {
                continue;
            }
            let sealed: SealedSession =
                serde_json::from_value(wrapper["sealed"].clone()).map_err(|_| Error::Forbidden)?;
            let original_key =
                KeyId::from_cipher_id(sealed.key_id()).map_err(|_| Error::NativeKeyUnavailable)?;
            if original_key.instance != target.instance || original_key == target {
                continue;
            }
            let scope: CheckpointScope =
                serde_json::from_value(wrapper["scope"].clone()).map_err(|_| Error::Forbidden)?;
            let run_payload: Value =
                parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let binding = run_payload["spec"]["input"]["frozen_mcp_bindings"]
                .as_array()
                .and_then(|a| a.iter().find(|b| b["entity_id"] == entity.to_string()))
                .ok_or(Error::Forbidden)?;
            if scope.project_id != id(&r, "project_id")?.to_string()
                || scope.run_id != run.to_string()
                || scope.server_id != entity.to_string()
                || scope.connection_record_id != format!("{run}:{entity}")
                || scope.checkpoint_revision
                    != s.try_get::<i64, _>("revision").map_err(storage_error)?
                || scope.configuration_sha256
                    != s.try_get::<String, _>("binding_sha")
                        .map_err(storage_error)?
                || scope.catalog_sha256
                    != s.try_get::<String, _>("catalog_sha")
                        .map_err(storage_error)?
                || binding["revision"] != scope.server_revision
            {
                return Err(Error::Forbidden);
            }
            let cipher = SessionCipher::new(
                sealed.key_id(),
                keys.load(&original_key)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?;
            let new_cipher = SessionCipher::new(
                &target
                    .cipher_id()
                    .map_err(|_| Error::NativeKeyUnavailable)?,
                keys.load(&target)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?;
            wrapper["sealed"] = json!(
                cipher
                    .rewrap(&sealed, &scope, &new_cipher)
                    .map_err(|_| Error::Forbidden)?
            );
            one(sqlx::query("UPDATE android_private_mcp_sessions SET checkpoint=$1 WHERE run_id=$2 AND entity_id=$3 AND checkpoint=$4").bind(json_string(&wrapper)?).bind(run.to_string()).bind(entity.to_string()).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
            count += 1;
        }
    }
    let pushes=sqlx::query("SELECT id,identity_id,device_id,revision,active,sealed_token FROM android_push_subscriptions ORDER BY id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    bounded(pushes.len())?;
    for p in pushes {
        let original: String = p.try_get("sealed_token").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance || sealed.key_id() == target {
            continue;
        }
        let id: String = p.try_get("id").map_err(storage_error)?;
        let revision: i64 = p.try_get("revision").map_err(storage_error)?;
        let cipher_revision = if p.try_get::<i64, _>("active").map_err(storage_error)? == 1 {
            revision
        } else {
            revision.checked_sub(1).ok_or(Error::Forbidden)?
        };
        let scope = CredentialScope {
            project_id: p.try_get("identity_id").map_err(storage_error)?,
            server_id: p.try_get("device_id").map_err(storage_error)?,
            reference: format!("android-push:{id}"),
            revision: cipher_revision,
            endpoint: "fcm://subscription".into(),
        };
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let next =
            seal_bearer(keys, target, &scope, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        one(sqlx::query("UPDATE android_push_subscriptions SET sealed_token=$1 WHERE id=$2 AND revision=$3 AND sealed_token=$4").bind(json_string(&next)?).bind(id).bind(revision).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    bounded(usize::try_from(count).map_err(|_| Error::BudgetExceeded)?)?;
    Ok(count)
}
