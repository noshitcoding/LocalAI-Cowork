//! Public run convenience endpoints over the original actor-owned encrypted content.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use crate::android_private_server::{authority, id, row};
use crate::android_private_workspace as workspace;
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use zeroize::Zeroizing;
impl Store {
    pub async fn android_private_result_snapshot(
        &self,
        actor: &Principal,
        run: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        let project = id(&r, "project_id")?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let snapshot: String = sqlx::query_scalar(
            "SELECT snapshot_id FROM android_private_loop_results WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Conflict)?;
        let value = crate::android_private_snapshots::manifest(
            &mut tx,
            actor.identity_id(),
            project,
            Uuid::parse_str(&snapshot).map_err(storage_error)?,
            true,
            now,
        )
        .await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_private_result_file(
        &self,
        actor: &Principal,
        run: Uuid,
        path: &str,
        keys: &dyn KeyStore,
    ) -> Result<Zeroizing<Vec<u8>>> {
        crate::android_private_snapshots::relative(path)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        let project = id(&r, "project_id")?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let snapshot: String = sqlx::query_scalar(
            "SELECT snapshot_id FROM android_private_loop_results WHERE run_id=$1",
        )
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Conflict)?;
        let bytes = crate::android_private_snapshots::file_bytes(
            &mut tx,
            actor.identity_id(),
            project,
            Uuid::parse_str(&snapshot).map_err(storage_error)?,
            path,
            keys,
            now,
        )
        .await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
    /// Attachments are frozen before the original worker claim; late uploads cannot change an admitted prompt.
    pub async fn android_private_upload_attachment(
        &self,
        actor: &Principal,
        run: Uuid,
        name: &str,
        media: &str,
        bytes: &[u8],
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Value> {
        if name.is_empty()
            || name.len() > 200
            || name.contains(['/', '\\', ':'])
            || name.chars().any(char::is_control)
            || matches!(name, "." | "..")
            || media.is_empty()
            || media.len() > 256
            || media.chars().any(char::is_control)
            || bytes.is_empty()
            || bytes.len() > 8 * 1024 * 1024
        {
            return Err(device_invalid());
        }
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        let project = id(&r, "project_id")?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        let old:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_private_loop_artifacts WHERE run_id=$1 ORDER BY id LIMIT 1001").bind(run.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if old.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        for value in old {
            let v: Value = parse(&value)?;
            if v["kind"] == "attachment"
                && v["name"] == name
                && v["media_type"] == media
                && v["digest"] == hash(bytes)
            {
                tx.commit().await.map_err(storage_error)?;
                return Ok(v);
            }
        }
        if !matches!(
            r.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "queued" | "waiting_for_snapshot"
        ) {
            return Err(Error::Conflict);
        }
        authority(&mut tx, &r, now).await?;
        let artifact = Uuid::new_v4();
        let version = workspace::add_version(
            &mut tx,
            &r,
            &format!("attachments/{artifact}/{name}"),
            "artifact",
            bytes,
            keys,
            key_id,
            now,
        )
        .await?;
        let metadata = json!({"schema_version":2,"id":artifact,"run_id":run,"revision":1,"kind":"attachment","media_type":media,"name":name,"digest":hash(bytes),"size_bytes":bytes.len(),"metadata":{"adapter":"android_private_loop"},"created_at":device_iso(now)?,"deleted_at":null});
        sqlx::query("INSERT INTO android_private_loop_artifacts(id,run_id,version_id,payload) VALUES($1,$2,$3,$4)").bind(artifact.to_string()).bind(run.to_string()).bind(version).bind(json_string(&metadata)?).execute(&mut *tx).await.map_err(storage_error)?;
        workspace::bounds(&mut tx, run).await?;
        let now = device_writer(&mut tx).await?;
        authority(&mut tx, &r, now).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(metadata)
    }
}
