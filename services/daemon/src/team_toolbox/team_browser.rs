//! One physical browser context across the approved steps of a live team run.
use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
pub(super) struct BrowserState {
    run: Uuid,
    team: Uuid,
    order: Uuid,
    lease: Mutex<Option<Arc<WorkspaceLease>>>,
    guard: Mutex<Option<containers::Guard>>,
    gate: tokio::sync::Mutex<()>,
    stopped: AtomicBool,
    last_used: Mutex<std::time::Instant>,
}
impl BrowserState {
    fn finish(&self) -> Result<Option<(Uuid, Vec<u8>)>> {
        self.stopped.store(true, Ordering::Release);
        let removed = if let Some(guard) = self.guard.lock().map_err(|_| Error::Conflict)?.take() {
            let nonce = guard.nonce()?;
            let original = guard.original_bytes()?;
            guard.finish()?;
            Some((nonce, original))
        } else {
            None
        };
        self.lease.lock().map_err(|_| Error::Conflict)?.take();
        Ok(removed)
    }
}
impl TeamToolbox {
    pub async fn shutdown(&self) {
        let states = std::mem::take(&mut *self.browsers.lock().unwrap_or_else(|e| e.into_inner()));
        for (_, s) in states {
            s.stopped.store(true, Ordering::Release);
        }
        let tasks =
            std::mem::take(&mut *self.browser_tasks.lock().unwrap_or_else(|e| e.into_inner()));
        for task in tasks {
            let _ = task.await;
        }
    }
    async fn browser_job(
        &self,
        authority: &TeamExecutionAuthority,
        request: cowork_toolbox::Request,
        lease: Arc<WorkspaceLease>,
    ) -> Result<cowork_toolbox::Job> {
        let order = authority.order_id().to_string();
        authority.revalidate().await?;
        let job = self
            .toolbox
            .submit_with_lease(&order, request, lease)
            .map_err(tool_error)?;
        let _guard = NativeJobGuard {
            toolbox: self.toolbox.clone(),
            order: order.clone(),
            job: job.id.clone(),
        };
        let started = tokio::time::Instant::now();
        loop {
            let job = self.toolbox.status(&order, &job.id).map_err(tool_error)?;
            if job.state == "completed" {
                let outcome = job.outcome.as_ref().ok_or(Error::UncertainEffect)?;
                if outcome.exit_code != Some(0) || !outcome.output_complete {
                    return Err(Error::UncertainEffect);
                }
                return Ok(job);
            }
            authority.revalidate().await?;
            if job.state != "running" || started.elapsed() > Duration::from_secs(130) {
                return Err(Error::UncertainEffect);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    pub(super) async fn perform_browser(
        &self,
        authority: TeamExecutionAuthority,
        arguments: Value,
    ) -> Result<Value> {
        let containers = self.containers.as_ref().ok_or(Error::Forbidden)?;
        let v: Browser = serde_json::from_value(arguments).map_err(|_| invalid())?;
        if !["navigate", "fill", "click", "dom", "screenshot", "close"]
            .contains(&v.operation.as_str())
        {
            return Err(invalid());
        }
        if v.operation == "navigate" && v.url.is_none() {
            return Err(invalid());
        }
        if let Some(url) = &v.url {
            let parsed = reqwest::Url::parse(url).map_err(|_| invalid())?;
            if !["http", "https"].contains(&parsed.scheme())
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || url.len() > 8192
            {
                return Err(invalid());
            }
        }
        if ["fill", "click"].contains(&v.operation.as_str())
            && v.selector
                .as_ref()
                .is_none_or(|s| s.is_empty() || s.len() > 4096)
        {
            return Err(invalid());
        }
        if v.operation == "fill" && v.text.as_ref().is_none_or(|s| s.len() > 32768) {
            return Err(invalid());
        }
        let id = v
            .session_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Uuid::parse_str(&id).map_err(|_| invalid())?;
        let action = json!({"session_id":id,"operation":v.operation,"url":v.url,"selector":v.selector,"text":v.text});
        self.browsers
            .lock()
            .map_err(|_| Error::Conflict)?
            .retain(|_, s| !s.stopped.load(Ordering::Acquire));
        let old = self
            .browsers
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(&id)
            .cloned();
        if v.session_id.is_some() && old.is_none() {
            return Err(Error::Validation(
                "Browser context ended; explicitly navigate a new session".into(),
            ));
        }
        if old.as_ref().is_some_and(|s| {
            s.run != authority.run_id()
                || s.team != authority.team_id()
                || s.order != authority.order_id()
        }) {
            return Err(Error::Forbidden);
        }
        if old.is_none() && v.operation != "navigate" {
            return Err(invalid());
        }
        let state = if let Some(state) = old {
            state
        } else {
            let (mut record, mut request) = containers.prepare(
                authority.team_id(),
                authority.order_id(),
                authority.run_id(),
                "browser_session",
                json!({"session_id":id}),
                None,
            )?;
            let preview = self
                .toolbox
                .preview(&authority.order_id().to_string(), &request)
                .map_err(tool_error)?;
            let approval=authority.request_tool_approval("browser",json!({"mode":"team_persistent_browser","action":action,"container":record.public()}),preview.request_sha256.clone()).await?;
            self.browser_approval(&authority, approval.id, &preview.request_sha256)
                .await?;
            let lease = self
                .toolbox
                .workspace_lease(&authority.order_id().to_string())
                .map_err(tool_error)?;
            record.admit(&authority, approval.id).await?;
            let nonce = record.nonce();
            let mut guard = containers.guard(record, lease.clone())?;
            request.approval_sha256 = Some(preview.request_sha256);
            guard.check_original_host()?;
            authority
                .dispatch_container_host(approval.id, nonce)
                .await?;
            let created = self.browser_job(&authority, request, lease.clone()).await?;
            let start = guard.created_background(&created)?;
            authority
                .receive_container_host(nonce, &guard.original_bytes()?)
                .await?;
            authority.revalidate_tool_approval(approval.id).await?;
            guard.check_original_host()?;
            authority.start_container_host(approval.id, nonce).await?;
            let started = self.browser_job(&authority, start, lease.clone()).await?;
            guard.started(started.id.clone());
            authority
                .finish_tool_approval(approval.id, Some(started.id), "completed", None)
                .await?;
            let state = Arc::new(BrowserState {
                run: authority.run_id(),
                team: authority.team_id(),
                order: authority.order_id(),
                lease: Mutex::new(Some(lease)),
                guard: Mutex::new(Some(guard)),
                gate: tokio::sync::Mutex::new(()),
                stopped: AtomicBool::new(false),
                last_used: Mutex::new(std::time::Instant::now()),
            });
            self.browsers
                .lock()
                .map_err(|_| Error::Conflict)?
                .insert(id.clone(), state.clone());
            let watched = state.clone();
            let live = authority.clone();
            let task = tokio::spawn(async move {
                loop {
                    let expired = watched
                        .last_used
                        .lock()
                        .map(|t| t.elapsed() > Duration::from_secs(300))
                        .unwrap_or(true);
                    if watched.stopped.load(Ordering::Acquire)
                        || expired
                        || live.revalidate().await.is_err()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                if let Ok(Ok(Some((nonce, raw)))) =
                    tokio::task::spawn_blocking(move || watched.finish()).await
                {
                    let _ = live.container_host_removed(nonce, &raw).await;
                }
            });
            self.browser_tasks
                .lock()
                .map_err(|_| Error::Conflict)?
                .retain(|t| !t.is_finished());
            self.browser_tasks
                .lock()
                .map_err(|_| Error::Conflict)?
                .push(task);
            state
        };
        let _gate = state.gate.lock().await;
        if state.stopped.load(Ordering::Acquire) {
            return Err(Error::UncertainEffect);
        }
        *state.last_used.lock().map_err(|_| Error::Conflict)? = std::time::Instant::now();
        let (mut request, container, nonce) = {
            let guard = state.guard.lock().map_err(|_| Error::Conflict)?;
            let guard = guard.as_ref().ok_or(Error::UncertainEffect)?;
            (
                guard.browser_request(action.clone())?,
                guard.public()?,
                guard.nonce()?,
            )
        };
        let preview = self
            .toolbox
            .preview(&authority.order_id().to_string(), &request)
            .map_err(tool_error)?;
        let approval = authority
            .request_tool_approval(
                "browser",
                json!({"mode":"team_persistent_browser","action":action,"container":container}),
                preview.request_sha256.clone(),
            )
            .await?;
        self.browser_approval(&authority, approval.id, &preview.request_sha256)
            .await?;
        request.approval_sha256 = Some(preview.request_sha256);
        let lease = state
            .lease
            .lock()
            .map_err(|_| Error::Conflict)?
            .as_ref()
            .cloned()
            .ok_or(Error::UncertainEffect)?;
        authority.revalidate_container_host(nonce).await?;
        let result = self.browser_job(&authority, request, lease).await;
        let job = match result {
            Ok(job) => job,
            Err(e) => {
                let s = state.clone();
                if let Ok(Ok(Some((nonce, raw)))) =
                    tokio::task::spawn_blocking(move || s.finish()).await
                {
                    let _ = authority.container_host_removed(nonce, &raw).await;
                }
                let _ = authority
                    .finish_tool_approval(
                        approval.id,
                        None,
                        "unknown",
                        Some("browser_outcome_unconfirmed".into()),
                    )
                    .await;
                return Err(e);
            }
        };
        let stdout = &job
            .outcome
            .as_ref()
            .ok_or(Error::UncertainEffect)?
            .stdout
            .bytes;
        let result: Value = serde_json::from_slice(stdout).map_err(|_| Error::UncertainEffect)?;
        let value =
            json!({"job_id":job.id,"state":"completed","browser_session_id":id,"result":result});
        authority.record_tool_result(&value).await?;
        authority.revalidate_tool_approval(approval.id).await?;
        authority
            .finish_tool_approval(approval.id, Some(job.id.clone()), "completed", None)
            .await?;
        if v.operation == "close" {
            let s = state.clone();
            if let Some((nonce, raw)) = tokio::task::spawn_blocking(move || s.finish())
                .await
                .map_err(|_| Error::UncertainEffect)??
            {
                authority.container_host_removed(nonce, &raw).await?;
            }
        }
        Ok(value)
    }
    async fn browser_approval(
        &self,
        authority: &TeamExecutionAuthority,
        id: Uuid,
        digest: &str,
    ) -> Result<()> {
        let start = tokio::time::Instant::now();
        loop {
            authority.revalidate().await?;
            let approval = authority.tool_approval(id).await?;
            match approval.state.as_str() {
                "approved" => break,
                "pending" => {}
                _ => return Err(Error::Forbidden),
            }
            if start.elapsed() > Duration::from_secs(180) {
                return Err(Error::Forbidden);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        authority.claim_tool_approval(id, digest).await?;
        authority.revalidate_tool_approval(id).await
    }
}
