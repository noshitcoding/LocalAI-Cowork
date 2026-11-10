use super::*;

impl Runtime {
    async fn readable_run_copy(&self, order: &str, run: &str) -> Result<RunRecord> {
        let owner = self.store.order(order).await?;
        let record = self.store.stored_run(run).await?.record;
        if record.spec.order_id != owner.id
            || record.spec.project_id != owner.project_id
            || record.spec.input_workspace.is_none()
        {
            return Err(Error::Forbidden);
        }
        self.restore_run_workspace(&record).await?;
        Ok(record)
    }
    pub async fn run_copy_files(
        &self,
        order: &str,
        run: &str,
        location: FileLocation,
    ) -> Result<DirectoryListing> {
        let record = self.readable_run_copy(order, run).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let permit = self.begin_upload_transfer()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.list(&record, &location)
        })
        .await
        .map_err(storage_error)?
    }
    pub async fn run_copy_text(
        &self,
        order: &str,
        run: &str,
        location: FileLocation,
    ) -> Result<TextFile> {
        let record = self.readable_run_copy(order, run).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let permit = self.begin_upload_transfer()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.read(&record, &location)
        })
        .await
        .map_err(storage_error)?
    }
    pub async fn download_workspace(
        &self,
        order: &str,
        input: DownloadFile,
    ) -> Result<cowork_broker::BinaryFile> {
        let workspace = self.workspace(order).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.download_workspace(&workspace, &input))
            .await
            .map_err(storage_error)?
    }

    pub async fn download_snapshot(
        &self,
        order: &str,
        run: &str,
        input: DownloadFile,
    ) -> Result<cowork_broker::BinaryFile> {
        let snapshot = self.retained_workspace_snapshot(order, run).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.download_snapshot(&snapshot, &input))
            .await
            .map_err(storage_error)?
    }

    async fn retained_workspace_snapshot(
        &self,
        order: &str,
        run: &str,
    ) -> Result<WorkspaceSnapshot> {
        let record = self.store.stored_run(run).await?.record;
        let owner = self.store.order(order).await?;
        if record.spec.order_id != owner.id || record.spec.project_id != owner.project_id {
            return Err(Error::Forbidden);
        }
        let snapshot = self
            .store
            .workspace_snapshot(run)
            .await?
            .ok_or(Error::NotFound)?;
        if snapshot.order_id != owner.id || snapshot.run_id != record.id {
            return Err(Error::Forbidden);
        }
        Ok(snapshot)
    }

    pub async fn workspace_transfer_manifest(
        &self,
        order: &str,
        run: &str,
    ) -> Result<WorkspaceTransferManifest> {
        let snapshot = self.retained_workspace_snapshot(order, run).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let permit = self.begin_upload_transfer()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.workspace_transfer_manifest(&snapshot)
        })
        .await
        .map_err(storage_error)?
    }

    pub async fn workspace_transfer_chunk(
        &self,
        order: &str,
        run: &str,
        input: ReadWorkspaceTransferChunk,
    ) -> Result<cowork_broker::BinaryFile> {
        let snapshot = self.retained_workspace_snapshot(order, run).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let permit = self.begin_upload_transfer()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.workspace_transfer_chunk(&snapshot, &input)
        })
        .await
        .map_err(storage_error)?
    }
    pub(super) async fn restore_run_workspace(&self, run: &RunRecord) -> Result<()> {
        if let Some(binding) = &run.spec.input_workspace {
            let custody = self.store.run_workspace_custody(binding).await?;
            let record: cowork_broker::RunWorkspaceRecord =
                serde_json::from_value(custody).map_err(storage_error)?;
            let files = self.files.clone().ok_or(Error::Forbidden)?;
            let spec = run.spec.clone();
            return tokio::task::spawn_blocking(move || {
                files.restore_run_workspace_record(&record, &spec)
            })
            .await
            .map_err(storage_error)?;
        }
        let mut managed = run.spec.folders.iter().filter(|f| f.order_id.is_some());
        if let Some(grant) = managed.next() {
            let workspace = self.workspace(&run.spec.order_id).await?;
            // A run may narrow a writable workspace to read-only. All identity and
            // revision fields must still match, and narrowing must never elevate rights.
            let mut expected = workspace.folder.clone();
            expected.writable = grant.writable;
            if managed.next().is_some()
                || *grant != expected
                || (grant.writable && !workspace.folder.writable)
            {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    pub async fn workspace_files(&self, order: &str, path: String) -> Result<DirectoryListing> {
        let workspace = self.workspace(order).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.list_workspace(&workspace, &path))
            .await
            .map_err(storage_error)?
    }
    pub async fn workspace_text(&self, order: &str, path: String) -> Result<TextFile> {
        let workspace = self.workspace(order).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.read_workspace(&workspace, &path))
            .await
            .map_err(storage_error)?
    }
    pub async fn workspace_stat(&self, order: &str, path: String) -> Result<FileStat> {
        let workspace = self.workspace(order).await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.stat_workspace(&workspace, &path))
            .await
            .map_err(storage_error)?
    }

    pub async fn workspace(&self, order_id: &str) -> Result<Workspace> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let order = self.store.order(order_id).await?;
        let base = self.store.workspace_base(order_id).await?;
        let expected = self.store.workspace(order_id).await?;
        let workspace = tokio::task::spawn_blocking(move || {
            // Read-only UI polling also resolves this workspace and briefly takes
            // its initialization lock. A run resuming after approval must not fail
            // merely because one of those lookups is in progress. WorkspaceBusy
            // here occurs before ensure_workspace changes anything; no tool or
            // publication is retried. Identity/origin errors still return at once.
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            loop {
                match files.ensure_workspace(&order, base.as_ref(), expected.as_ref()) {
                    Err(Error::WorkspaceBusy) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    result => return result,
                }
            }
        })
        .await
        .map_err(storage_error)??;
        self.store.register_workspace(&workspace).await?;
        Ok(workspace)
    }

    /// Completion content is already checkpointed. Resuming this phase never calls the provider.
    pub(super) async fn finish_workspace(
        &self,
        run_id: &str,
        token: &str,
        stop: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let stored = self
            .store
            .mutate(run_id, Some(token), |r| {
                if !r.checkpoint.completion_pending || r.checkpoint.in_flight.is_some() {
                    return Err(Error::Conflict);
                }
                if r.record.state == RunState::Running {
                    r.transition(RunState::WaitingForSnapshot)?;
                }
                if r.record.state != RunState::WaitingForSnapshot {
                    return Err(Error::Conflict);
                }
                Ok(Change::event(
                    "workspace_snapshot_started",
                    json!({"state":"waiting_for_snapshot"}),
                ))
            })
            .await?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let snapshot =
            tokio::task::spawn_blocking(move || files.snapshot_workspace(&stored.record));
        tokio::pin!(snapshot);
        let mut heartbeat = tokio::time::interval(Duration::from_millis(250));
        let mut last_heartbeat = tokio::time::Instant::now();
        let prepared = loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => return Err(Error::Provider("shutdown".into())),
                _ = tokio::time::sleep_until(deadline) => return Err(Error::BudgetExceeded),
                _ = heartbeat.tick() => {
                    let current = self.store.stored_run(run_id).await?;
                    if self.ack_cancel(run_id,token,&current).await? { return Ok(()); }
                    if time_budget_exhausted(&current.record,now()) { return Err(Error::BudgetExceeded); }
                    if last_heartbeat.elapsed() >= Duration::from_secs(3) { self.store.heartbeat(run_id,token).await?; last_heartbeat=tokio::time::Instant::now(); }
                }
                result = &mut snapshot => break result.map_err(storage_error)??,
            }
        };
        self.store
            .mutate(run_id, Some(token), |r| {
                if r.record.state != RunState::WaitingForSnapshot
                    || !r.checkpoint.completion_pending
                {
                    return Err(Error::Conflict);
                }
                r.transition(RunState::Completed)?;
                r.checkpoint.completion_pending = false;
                r.lease = None;
                let mut change = Change::event(
                    "run_completed",
                    json!({"state":"completed","workspace_sha256":prepared.snapshot.sha256}),
                );
                change.workspace_snapshot = Some(prepared.snapshot.clone());
                change.messages.push(Message {
                    id: id(),
                    order_id: r.record.spec.order_id.clone(),
                    run_id: run_id.into(),
                    role: "assistant".into(),
                    content: r.checkpoint.last_model_content.clone(),
                    created_at: now(),
                });
                Ok(change)
            })
            .await?;
        // Retain the physical read lock until the transaction above releases the durable owner.
        drop(prepared);
        Ok(())
    }
}
