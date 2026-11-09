use super::*;
use cowork_domain::file_watch::{
    FileWatchState, acknowledge_file, observe_files, observe_files_with_unavailable,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedWatch {
    state: FileWatchState,
    status: AutomationFileWatchStatus,
}

pub struct FileWatchClaim {
    pub trigger: AutomationTrigger,
    lease_id: String,
    revision: i64,
}

async fn save(tx: &mut Transaction<'_, Any>, saved: &SavedWatch) -> Result<()> {
    let payload = json_string(saved)?;
    if payload.len() > 64 * 1024 * 1024 {
        return Err(Error::Validation("automation_watch_state_limit".into()));
    }
    sqlx::query(
        "UPDATE automation_file_watches SET next_scan_at=$1,payload=$2 WHERE trigger_id=$3",
    )
    .bind(saved.status.next_scan_at)
    .bind(payload)
    .bind(&saved.status.trigger_id)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}

pub(super) async fn configure_in_tx(
    tx: &mut Transaction<'_, Any>,
    current: &AutomationTrigger,
    previous: Option<&AutomationTrigger>,
    baseline: Option<&[FileWatchObservation]>,
    ignored_entries: u32,
) -> Result<()> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT payload FROM automation_file_watches WHERE trigger_id=$1")
            .bind(&current.id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    let same = previous.is_some_and(|previous| {
        previous.file_watch == current.file_watch && previous.source == current.source
    });
    let prior: Option<SavedWatch> = if same {
        raw.map(|raw| parse(&raw)).transpose()?
    } else {
        None
    };
    if current.source != AutomationEventSource::File {
        sqlx::query("DELETE FROM automation_file_watches WHERE trigger_id=$1")
            .bind(&current.id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        return Ok(());
    }
    let mut state = prior
        .as_ref()
        .map_or_else(FileWatchState::default, |prior| prior.state.clone());
    if !same && let Some(files) = baseline {
        let watch = current.file_watch.as_ref().ok_or(Error::Forbidden)?;
        if watch.include_existing {
            return Err(Error::Forbidden);
        }
        observe_files(&mut state, watch, files, now())?;
    }
    let saved = SavedWatch {
        status: AutomationFileWatchStatus {
            trigger_id: current.id.clone(),
            trigger_revision: current.revision,
            initialized: state.initialized,
            tracked_files: state.files.len() as u32,
            pending_changes: state
                .files
                .values()
                .filter(|file| file.pending.is_some())
                .count() as u32,
            ignored_entries: prior
                .as_ref()
                .map_or(ignored_entries, |prior| prior.status.ignored_entries),
            last_scan_at: state.last_observed_at,
            next_scan_at: now(),
            scanning: false,
            reason: if current.enabled {
                None
            } else {
                Some("trigger_paused".into())
            },
        },
        state,
    };
    sqlx::query("INSERT INTO automation_file_watches(trigger_id,trigger_revision,revision,next_scan_at,lease_id,lease_until,payload) VALUES($1,$2,1,$3,NULL,0,$4) ON CONFLICT(trigger_id) DO UPDATE SET trigger_revision=excluded.trigger_revision,revision=automation_file_watches.revision+1,next_scan_at=excluded.next_scan_at,lease_id=NULL,lease_until=0,payload=excluded.payload")
        .bind(&current.id).bind(current.revision).bind(saved.status.next_scan_at).bind(json_string(&saved)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

impl Store {
    pub async fn automation_trigger_original(
        &self,
        project: &str,
        input: &WriteAutomationTrigger,
    ) -> Result<Option<AutomationTrigger>> {
        valid_text(&input.idempotency_key, 128)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        crate::workspace_management::original(
            &mut tx,
            &format!("automation-trigger:{project}"),
            &input.idempotency_key,
            &canonical_hash(input),
        )
        .await
    }

    pub async fn automation_file_watch_status(
        &self,
        project: &str,
        key: &str,
    ) -> Result<AutomationFileWatchStatus> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let current = trigger(&mut tx, project, key).await?;
        if current.source != AutomationEventSource::File {
            return Err(Error::NotFound);
        }
        let raw: String =
            sqlx::query_scalar("SELECT payload FROM automation_file_watches WHERE trigger_id=$1")
                .bind(key)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        Ok(parse::<SavedWatch>(&raw)?.status)
    }

    /// Claims only observation IO. A dead observer's lease can expire because it
    /// has no authority to emit events after its lease/revision has been replaced.
    pub async fn claim_automation_file_watch(&self) -> Result<Option<FileWatchClaim>> {
        let rows = sqlx::query("SELECT t.id,t.project_id FROM automation_file_watches w JOIN automation_triggers t ON t.id=w.trigger_id WHERE t.enabled=1 AND w.next_scan_at<=$1 AND w.lease_until<=$1 AND NOT EXISTS(SELECT 1 FROM native_project_retirements p WHERE p.project_id=t.project_id) ORDER BY w.next_scan_at,t.id LIMIT 32")
            .bind(now()).fetch_all(&self.pool).await.map_err(storage_error)?;
        for row in rows {
            let project: String = row.try_get("project_id").map_err(storage_error)?;
            let key: String = row.try_get("id").map_err(storage_error)?;
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            match crate::workspace_management::project_lock(&mut tx, &project).await {
                Ok(_) => {}
                Err(Error::NotFound) => continue,
                Err(error) => return Err(error),
            }
            let current = trigger(&mut tx, &project, &key).await?;
            let Some(watch) = &current.file_watch else {
                continue;
            };
            let row = sqlx::query("SELECT revision,payload FROM automation_file_watches WHERE trigger_id=$1 AND trigger_revision=$2 AND next_scan_at<=$3 AND lease_until<=$3")
                .bind(&key).bind(current.revision).bind(now()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let Some(row) = row else {
                continue;
            };
            if !current.enabled {
                continue;
            }
            let mut saved: SavedWatch =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let task = task(&mut tx, &project, &current.task_id).await?;
            let mut reason = if task.archived || task.revision != current.task_revision {
                Some("task_changed")
            } else {
                None
            };
            if reason.is_none() {
                match crate::workspace_management::order_lock(&mut tx, &task.value.order_id).await {
                    Ok(order) => {
                        if order.project_id != project
                            || crate::workspace_management::archived(&mut tx, &order.id).await?
                        {
                            reason = Some("chat_unavailable");
                        }
                    }
                    Err(Error::NotFound) => reason = Some("chat_unavailable"),
                    Err(error) => return Err(error),
                }
            }
            if let Some(reason) = reason {
                saved.status.reason = Some(reason.into());
                saved.status.scanning = false;
                saved.status.next_scan_at = now().saturating_add(i64::from(watch.poll_interval_ms));
                save(&mut tx, &saved).await?;
                tx.commit().await.map_err(storage_error)?;
                continue;
            }
            let revision = row
                .try_get::<i64, _>("revision")
                .map_err(storage_error)?
                .checked_add(1)
                .ok_or(Error::Conflict)?;
            let lease_id = id();
            saved.status.scanning = true;
            saved.status.reason = None;
            sqlx::query("UPDATE automation_file_watches SET revision=$1,lease_id=$2,lease_until=$3,payload=$4 WHERE trigger_id=$5")
                .bind(revision).bind(&lease_id).bind(now().saturating_add(60_000)).bind(json_string(&saved)?).bind(&key).execute(&mut *tx).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(FileWatchClaim {
                trigger: current,
                lease_id,
                revision,
            }));
        }
        Ok(None)
    }

    pub async fn complete_automation_file_watch(
        &self,
        claim: &FileWatchClaim,
        files: &[FileWatchObservation],
        ignored_entries: u32,
        unavailable_paths: &[String],
    ) -> Result<usize> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::workspace_management::project_lock(&mut tx, &claim.trigger.project_id).await?;
        let current = trigger(&mut tx, &claim.trigger.project_id, &claim.trigger.id).await?;
        if !current.enabled || current.revision != claim.trigger.revision {
            return Err(Error::Conflict);
        }
        let task = task(&mut tx, &current.project_id, &current.task_id).await?;
        if task.archived || task.revision != current.task_revision {
            return Err(Error::Conflict);
        }
        let order = crate::workspace_management::order_lock(&mut tx, &task.value.order_id).await?;
        if order.project_id != current.project_id
            || crate::workspace_management::archived(&mut tx, &order.id).await?
        {
            return Err(Error::Conflict);
        }
        let row = sqlx::query("SELECT payload FROM automation_file_watches WHERE trigger_id=$1 AND revision=$2 AND lease_id=$3 AND lease_until>$4")
            .bind(&current.id).bind(claim.revision).bind(&claim.lease_id).bind(now()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let watch = current.file_watch.as_ref().ok_or(Error::Forbidden)?;
        let mut saved: SavedWatch =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let stamp = now();
        let changes = observe_files_with_unavailable(
            &mut saved.state,
            watch,
            files,
            unavailable_paths,
            stamp,
        )?;
        for change in &changes {
            let after = change.after.as_ref();
            let event = SubmitAutomationEvent {
                event_id: format!("file:{}", change.event_id),
                data: json!({"change":change.change,"folder_id":watch.folder.id,"resource_id":watch.folder.resource_id,
                    "path":change.path,"sha256":after.map(|file|file.sha256.as_str()),"previous_sha256":change.before.as_ref().map(|file|file.sha256.as_str()),
                    "size":after.map(|file|file.size)}),
            };
            admit_event_in_tx(&mut tx, &current, AutomationEventSource::File, &event).await?;
            acknowledge_file(&mut saved.state, change)?;
        }
        saved.status.initialized = saved.state.initialized;
        saved.status.tracked_files = saved.state.files.len() as u32;
        saved.status.pending_changes = saved
            .state
            .files
            .values()
            .filter(|file| file.pending.is_some())
            .count() as u32;
        saved.status.ignored_entries = ignored_entries;
        saved.status.last_scan_at = Some(stamp);
        saved.status.next_scan_at = stamp.saturating_add(i64::from(watch.poll_interval_ms));
        saved.status.scanning = false;
        saved.status.reason = if unavailable_paths.is_empty() {
            None
        } else {
            Some("automation_watch_entries_unavailable".into())
        };
        save(&mut tx, &saved).await?;
        sqlx::query("UPDATE automation_file_watches SET revision=revision+1,lease_id=NULL,lease_until=0 WHERE trigger_id=$1").bind(&current.id).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(changes.len())
    }

    /// A failed or partial scan preserves every observation and emits no removals.
    pub async fn fail_automation_file_watch(
        &self,
        claim: &FileWatchClaim,
        reason: &str,
    ) -> Result<()> {
        if reason.len() > 128 || reason.chars().any(char::is_control) {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::workspace_management::project_lock(&mut tx, &claim.trigger.project_id).await?;
        let row=sqlx::query("SELECT payload FROM automation_file_watches WHERE trigger_id=$1 AND revision=$2 AND lease_id=$3")
            .bind(&claim.trigger.id).bind(claim.revision).bind(&claim.lease_id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let Some(row) = row else {
            return Ok(());
        };
        let mut saved: SavedWatch =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let watch = claim.trigger.file_watch.as_ref().ok_or(Error::Forbidden)?;
        saved.status.reason = Some(reason.into());
        saved.status.scanning = false;
        saved.status.next_scan_at = now().saturating_add(i64::from(watch.poll_interval_ms));
        save(&mut tx, &saved).await?;
        sqlx::query("UPDATE automation_file_watches SET revision=revision+1,lease_id=NULL,lease_until=0 WHERE trigger_id=$1").bind(&claim.trigger.id).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
}
