use super::*;

impl Runtime {
    pub async fn write_automation_trigger(
        &self,
        project: &str,
        input: &WriteAutomationTrigger,
    ) -> Result<AutomationTrigger> {
        if let Some(original) = self
            .store
            .automation_trigger_original(project, input)
            .await?
        {
            return Ok(original);
        }
        cowork_domain::file_watch::validate_file_watch(&input.source, input.file_watch.as_ref())?;
        if let Some(watch) = &input.file_watch {
            cowork_broker::relative_path(&watch.path, true)?;
            let files = self.files.clone().ok_or(Error::Forbidden)?;
            let project_id = project.to_owned();
            let folder = watch.folder.clone();
            tokio::task::spawn_blocking(move || files.validate_grants(&project_id, &[folder]))
                .await
                .map_err(storage_error)??;
            let same = match input.id.as_deref() {
                Some(key) => self
                    .store
                    .automation_triggers(project)
                    .await?
                    .into_iter()
                    .find(|old| old.id == key)
                    .is_some_and(|old| {
                        old.file_watch == input.file_watch && old.source == input.source
                    }),
                None => false,
            };
            if !same && !watch.include_existing {
                let files = self.files.clone().ok_or(Error::Forbidden)?;
                let project_id = project.to_owned();
                let watch = watch.clone();
                let prepared = tokio::task::spawn_blocking(move || {
                    files.prepare_file_watch_scan(&project_id, &watch)
                })
                .await
                .map_err(storage_error)??;
                let result = self
                    .store
                    .write_automation_trigger_with_baseline(
                        project,
                        input,
                        Some(&prepared.files),
                        prepared.ignored_entries,
                    )
                    .await;
                drop(prepared);
                return result;
            }
        }
        self.store.write_automation_trigger(project, input).await
    }

    pub async fn poll_automation_file_watches(&self) -> Result<usize> {
        let mut events = 0;
        for _ in 0..2 {
            let Some(claim) = self.store.claim_automation_file_watch().await? else {
                break;
            };
            let outcome = if let Some(files) = self.files.clone() {
                let project = claim.trigger.project_id.clone();
                let watch = claim.trigger.file_watch.clone().ok_or(Error::Forbidden)?;
                match tokio::task::spawn_blocking(move || {
                    files.prepare_file_watch_scan(&project, &watch)
                })
                .await
                {
                    Ok(Ok(prepared)) => {
                        // The broker's read lock prevents revocation from overtaking this commit.
                        let result = self
                            .store
                            .complete_automation_file_watch(
                                &claim,
                                &prepared.files,
                                prepared.ignored_entries,
                                &prepared.unavailable_paths,
                            )
                            .await;
                        drop(prepared);
                        result
                    }
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(Error::Validation("automation_watch_worker_failed".into())),
                }
            } else {
                Err(Error::Validation(
                    "automation_watch_tools_unavailable".into(),
                ))
            };
            match outcome {
                Ok(count) => events += count,
                Err(error) => {
                    let reason = match &error {
                        Error::Validation(reason)
                            if reason.starts_with("automation_watch_")
                                || reason == "automation_event_retention_limit" =>
                        {
                            reason.as_str()
                        }
                        Error::Forbidden => "automation_watch_folder_unavailable",
                        Error::WorkspaceBusy => "automation_watch_folder_busy",
                        Error::FileConflict => "automation_watch_file_changed_during_scan",
                        Error::Conflict => "automation_watch_configuration_changed",
                        _ => "automation_watch_io_failed",
                    };
                    self.store
                        .fail_automation_file_watch(&claim, reason)
                        .await?;
                }
            }
        }
        Ok(events)
    }

    /// Persist a claim before IO. Errors only initiate original-run readback.
    pub async fn automation_tick(&self) -> Result<usize> {
        self.store.reconcile_automation_events().await?;
        let mut started = 0;
        for _ in 0..8 {
            let Some(dispatch) = self.store.claim_automation_event().await? else {
                break;
            };
            let result = self.create_run_request(&dispatch.request).await;
            self.store
                .reconcile_automation_event(&dispatch.event.id, result.is_err())
                .await?;
            if result.is_ok() {
                started += 1;
            }
        }
        self.poll_automation_file_watches().await?;
        Ok(started)
    }
}
