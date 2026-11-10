//! Native tool execution requires a separate durable, exact user approval.
//! The model cannot supply approval tokens or replace the server idempotency key.
use cowork_domain::{Error, Result};
use cowork_storage::team_service::{TeamExecutionAuthority, TeamToolDispatcher};
use cowork_toolbox::{Action, Toolbox, WorkspaceLease};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
#[path = "team_container.rs"]
mod containers;
mod team_browser;
pub use containers::ContainerConfig;
#[path = "team_github.rs"]
mod github_actions;

pub struct TeamToolbox {
    toolbox: Arc<Toolbox>,
    containers: Option<containers::Containers>,
    browsers: std::sync::Mutex<std::collections::BTreeMap<String, Arc<team_browser::BrowserState>>>,
    browser_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
// Dropping an admitted execution must stop this job even when an async database
// write or principal revalidation fails. Its durable state remains uncertain.
struct NativeJobGuard {
    toolbox: Arc<Toolbox>,
    order: String,
    job: String,
}
impl Drop for NativeJobGuard {
    fn drop(&mut self) {
        let _ = self.toolbox.cancel(&self.order, &self.job);
    }
}
impl TeamToolbox {
    pub fn new(toolbox: Arc<Toolbox>) -> Self {
        Self {
            toolbox,
            containers: None,
            browsers: Default::default(),
            browser_tasks: Default::default(),
        }
    }
    pub fn with_container(mut self, config: ContainerConfig) -> Result<Self> {
        self.containers = Some(containers::Containers::open(self.toolbox.clone(), config)?);
        Ok(self)
    }
    pub fn original_host_binding(&self) -> Result<cowork_contracts::TeamOriginalHostBinding> {
        self.containers
            .as_ref()
            .ok_or(Error::Forbidden)?
            .operator_binding()
    }
    pub fn recover_containers(&self) -> Result<()> {
        if let Some(containers) = &self.containers {
            containers.recover()?;
        }
        Ok(())
    }
    /// A retained unknown record blocks file IO even when its old worker lease
    /// has disappeared. This method performs no Docker IO on API processes.
    pub fn files_available(&self, order: Uuid) -> Result<()> {
        if let Some(containers) = &self.containers {
            containers.files_available(order)?;
        }
        Ok(())
    }
    pub fn file_workspace_lease(&self, order: Uuid) -> Result<Arc<WorkspaceLease>> {
        let lease = self
            .toolbox
            .workspace_lease(&order.to_string())
            .map_err(tool_error)?;
        self.files_available(order)?;
        Ok(lease)
    }
}
fn invalid() -> Error {
    Error::Validation("Invalid team tool request".into())
}
fn tool_error(_: cowork_toolbox::Error) -> Error {
    Error::UncertainEffect
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    program: String,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    cwd: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Browser {
    #[serde(default)]
    session_id: Option<String>,
    operation: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Office {
    operation: String,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    title: Option<String>,
}
fn valid_github_arguments(arguments: &[String]) -> bool {
    let Some(command) = arguments.first() else {
        return false;
    };
    if ![
        "api", "repo", "issue", "pr", "release", "workflow", "run", "search", "gist",
    ]
    .contains(&command.as_str())
        || arguments.len() > 128
        || arguments.iter().map(String::len).sum::<usize>() > 32768
    {
        return false;
    }
    if command == "repo"
        && !arguments.get(1).is_some_and(|s| {
            [
                "view",
                "list",
                "create",
                "edit",
                "delete",
                "rename",
                "archive",
                "unarchive",
            ]
            .contains(&s.as_str())
        })
    {
        return false;
    }
    if command == "pr"
        && arguments
            .get(1)
            .is_some_and(|s| ["checkout", "diff"].contains(&s.as_str()))
    {
        return false;
    }
    if command == "gist" && arguments.get(1).is_some_and(|s| s == "clone") {
        return false;
    }
    let denied = [
        "--",
        "--web",
        "--browser",
        "--verbose",
        "--hostname",
        "--header",
        "-H",
        "--input",
        "--config",
        "--git-protocol",
        "--with-token",
        "--clone",
        "--remote",
        "--source",
        "-s",
        "--push",
        "--body-file",
        "--templatefile",
        "--jq",
        "-q",
        "--template",
        "-t",
    ];
    let content = [
        "--title",
        "--body",
        "-b",
        "--raw-field",
        "-f",
        "--field",
        "-F",
    ];
    let mut content_value = false;
    for a in arguments {
        let inline_content = content
            .iter()
            .filter(|s| s.starts_with("--"))
            .any(|flag| a.starts_with(&format!("{flag}=")));
        if a.len() > 8192
            || a.chars().any(|c| {
                c.is_control()
                    && !((content_value || inline_content) && matches!(c, '\n' | '\r' | '\t'))
            })
        {
            return false;
        }
        if content_value {
            if a.starts_with('-') || a.starts_with('@') || a.contains("=@") {
                return false;
            }
            content_value = false;
            continue;
        }
        if denied.contains(&a.as_str())
            || denied
                .iter()
                .any(|flag| flag.starts_with("--") && a.starts_with(&format!("{flag}=")))
            || a.starts_with("-H")
            || a.starts_with("-q")
            || a.starts_with("-t")
            || a.starts_with("-s")
        {
            return false;
        }
        if content.contains(&a.as_str()) {
            content_value = true;
            continue;
        }
        if content.iter().filter(|s| s.starts_with("--")).any(|flag| {
            a.strip_prefix(&format!("{flag}="))
                .is_some_and(|v| !v.starts_with('@') && !v.contains("=@"))
        }) {
            continue;
        }
        let scheme = a.split_once(':').is_some_and(|(prefix, _)| {
            prefix.starts_with(|c: char| c.is_ascii_alphabetic())
                && prefix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
        });
        if scheme || a.contains("://") || a.starts_with("//") {
            return false;
        }
    }
    !content_value
}
impl TeamToolbox {
    fn definition(&self, name: &str) -> Result<Value> {
        let containers = self.containers.as_ref().ok_or(Error::Forbidden)?;
        if name == "github" && containers.network_enabled() {
            return Ok(github_actions::definition());
        }
        let (description, properties, required) = match name {
            "execute_program" => (
                "Request an explicitly approved program inside this team's isolated container and order workspace. No host program aliases or daemon credentials are available.",
                json!({"program":{"type":"string","enum":["python","node","git"]},"arguments":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"}}),
                vec!["program"],
            ),
            "browser" if containers.network_enabled() => (
                "Request an explicitly approved browser action in one live run-bound container. Navigate once, retain session_id for fill/click/DOM/screenshot, and close the session when done. Supplying a URL on an interaction verifies the live target without reloading the page. Sessions expire when the run ends, after five idle minutes, or after ten minutes.",
                json!({"session_id":{"type":"string"},"operation":{"type":"string","enum":["navigate","dom","screenshot","click","fill","close"]},"url":{"type":"string"},"selector":{"type":"string"},"text":{"type":"string"}}),
                vec!["operation"],
            ),
            "office" => (
                "Request an explicitly approved Word/Excel/PowerPoint/PDF action or configured Office conversion. All input/output paths are relative to this team's order workspace; format adapters require their configured Python libraries.",
                json!({"operation":{"type":"string","enum":["read_text","read_docx","read_xlsx","read_pptx","read_pdf","write_text","create_docx","create_xlsx","create_pptx","create_pdf","convert"]},"input":{"type":"string"},"output":{"type":"string"},"text":{"type":"string"},"title":{"type":"string"}}),
                vec!["operation"],
            ),
            _ => return Err(invalid()),
        };
        Ok(
            json!({"type":"function","function":{"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}}),
        )
    }
    async fn perform(
        &self,
        authority: TeamExecutionAuthority,
        name: String,
        arguments: Value,
    ) -> Result<Value> {
        self.definition(&name)?;
        if name == "browser" {
            return self.perform_browser(authority, arguments).await;
        }
        let containers = self.containers.as_ref().ok_or(Error::Forbidden)?;
        authority.revalidate().await?;
        let github = if name == "github" {
            Some(authority.github_metadata().await?.ok_or(Error::Forbidden)?)
        } else {
            None
        };
        let order = authority.order_id().to_string();
        let mut created_session = None;
        let guest = match name.as_str() {
            "execute_program" => {
                let v: Program = serde_json::from_value(arguments).map_err(|_| invalid())?;
                if !["python", "node", "git"].contains(&v.program.as_str()) {
                    return Err(invalid());
                }
                guest_action(Action::Terminal {
                    program: v.program,
                    arguments: v.arguments,
                    cwd: v.cwd,
                })?
            }
            "browser" => {
                let v: Browser = serde_json::from_value(arguments).map_err(|_| invalid())?;
                let url = v.url.as_ref().ok_or_else(invalid)?;
                let parsed = reqwest::Url::parse(url).map_err(|_| invalid())?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(invalid());
                }
                let session_id = match v.session_id {
                    Some(id) => Uuid::parse_str(&id).map_err(|_| invalid())?.to_string(),
                    None => {
                        let id = Uuid::new_v4().to_string();
                        created_session = Some(id.clone());
                        id
                    }
                };
                guest_action(Action::Browser {
                    session_id,
                    operation: v.operation,
                    url: v.url,
                    selector: v.selector,
                    text: v.text,
                })?
            }
            "office" => {
                let v: Office = serde_json::from_value(arguments).map_err(|_| invalid())?;
                guest_action(Action::Office {
                    operation: v.operation,
                    input: v.input,
                    output: v.output,
                    text: v.text,
                    title: v.title,
                })?
            }
            "github" => github_actions::request(
                arguments,
                &github.as_ref().ok_or(Error::Forbidden)?.hostname,
            )?,
            _ => return Err(invalid()),
        };
        let (mut record, mut request) = containers.prepare(
            authority.team_id(),
            authority.order_id(),
            authority.run_id(),
            &name,
            guest.clone(),
            github.as_ref().map(|g| (g.revision, g.hostname.clone())),
        )?;
        let preview = self.toolbox.preview(&order, &request).map_err(tool_error)?;
        let approval = authority
            .request_tool_approval(
                &name,
                json!({"mode":"team_container","tool":name,"action":guest,"container":record.public()}),
                preview.request_sha256.clone(),
            )
            .await?;
        let started = tokio::time::Instant::now();
        loop {
            authority.revalidate().await?;
            let current = authority.tool_approval(approval.id).await?;
            match current.state.as_str() {
                "approved" => break,
                "pending" => {}
                "rejected" | "expired" => {
                    return Err(Error::Forbidden);
                }
                _ => return Err(Error::UncertainEffect),
            }
            if started.elapsed() > Duration::from_secs(180) {
                let _ = authority
                    .finish_tool_approval(
                        approval.id,
                        None,
                        "expired",
                        Some("approval_wait_expired".into()),
                    )
                    .await;
                return Err(Error::Forbidden);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        authority
            .claim_tool_approval(approval.id, &preview.request_sha256)
            .await?;
        // Claim is durable before submission. A crash in this boundary leaves an
        // executing approval, not reusable approval for a second tool effect.
        authority.revalidate_tool_approval(approval.id).await?;
        request.approval_sha256 = Some(preview.request_sha256);
        let lease = self.toolbox.workspace_lease(&order).map_err(tool_error)?;
        authority.revalidate_tool_approval(approval.id).await?;
        record.admit(&authority, approval.id).await?;
        let nonce = record.nonce();
        let mut container_guard = containers.guard(record, lease.clone())?;
        if github.is_some() {
            let credential = authority
                .github_credential(approval.id)
                .await?
                .ok_or(Error::Forbidden)?;
            container_guard.github(credential)?;
        }
        authority.revalidate_tool_approval(approval.id).await?;
        container_guard.check_original_host()?;
        authority
            .dispatch_container_host(approval.id, nonce)
            .await?;
        let job = match self
            .toolbox
            .submit_with_lease(&order, request, lease.clone())
        {
            Ok(job) => job,
            Err(e) => {
                let _ = authority
                    .finish_tool_approval(
                        approval.id,
                        None,
                        "unknown",
                        Some("tool_submission_unconfirmed".into()),
                    )
                    .await;
                return Err(tool_error(e));
            }
        };
        let _create_guard = NativeJobGuard {
            toolbox: self.toolbox.clone(),
            order: order.clone(),
            job: job.id.clone(),
        };
        let started = tokio::time::Instant::now();
        let created = loop {
            authority.revalidate_tool_approval(approval.id).await?;
            let current = self.toolbox.status(&order, &job.id).map_err(tool_error)?;
            if current.state == "completed" {
                break current;
            }
            if current.state != "running" || started.elapsed() > Duration::from_secs(150) {
                return Err(Error::UncertainEffect);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let start_request = container_guard.created(&created)?;
        authority
            .receive_container_host(nonce, &container_guard.original_bytes()?)
            .await?;
        // The guest starts only after its exact created ID is durable and the
        // original principal, approver session, epoch and lease are checked again.
        authority.revalidate_tool_approval(approval.id).await?;
        container_guard.check_original_host()?;
        authority.start_container_host(approval.id, nonce).await?;
        let job = self
            .toolbox
            .submit_with_lease(&order, start_request, lease)
            .map_err(tool_error)?;
        container_guard.started(job.id.clone());
        let _job_guard = NativeJobGuard {
            toolbox: self.toolbox.clone(),
            order: order.clone(),
            job: job.id.clone(),
        };
        authority
            .finish_tool_approval(approval.id, Some(job.id.clone()), "executing", None)
            .await?;
        let started = tokio::time::Instant::now();
        loop {
            let state = self.toolbox.status(&order, &job.id).map_err(tool_error)?;
            if state.state == "completed" {
                let capture = |bytes: &[u8]| {
                    String::from_utf8_lossy(&bytes[..bytes.len().min(24 * 1024)]).into_owned()
                };
                let outcome=state.outcome.as_ref().map(|o|json!({"exit_code":o.exit_code,"termination":o.termination,"stdout":capture(&o.stdout.bytes),"stderr":capture(&o.stderr.bytes),"stdout_truncated":o.stdout.truncated||o.stdout.bytes.len()>24*1024,"stderr_truncated":o.stderr.truncated||o.stderr.bytes.len()>24*1024,"output_complete":o.output_complete,"duration_ms":o.duration_ms}));
                let value = json!({"job_id":state.id,"state":state.state,"outcome":outcome,"browser_session_id":created_session});
                authority.record_tool_result(&value).await?;
                let original_removed = container_guard.original_bytes()?;
                container_guard.finish()?;
                authority
                    .container_host_removed(nonce, &original_removed)
                    .await?;
                authority
                    .finish_tool_approval(approval.id, Some(job.id.clone()), "completed", None)
                    .await?;
                return Ok(value);
            }
            if let Err(e) = authority.revalidate_tool_approval(approval.id).await {
                let _ = self.toolbox.cancel(&order, &job.id);
                return Err(e);
            }
            if state.state != "running" {
                let _ = authority
                    .finish_tool_approval(
                        approval.id,
                        Some(job.id.clone()),
                        "unknown",
                        Some("tool_completion_unconfirmed".into()),
                    )
                    .await;
                return Err(Error::UncertainEffect);
            }
            if started.elapsed() > Duration::from_secs(150) {
                let _ = self.toolbox.cancel(&order, &job.id);
                let _ = authority
                    .finish_tool_approval(
                        approval.id,
                        Some(job.id.clone()),
                        "unknown",
                        Some("tool_completion_wait_expired".into()),
                    )
                    .await;
                return Err(Error::UncertainEffect);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}
fn guest_action(action: Action) -> Result<Value> {
    let mut value = serde_json::to_value(action).map_err(cowork_domain::storage_error)?;
    value.as_object_mut().ok_or_else(invalid)?.remove("kind");
    Ok(value)
}
impl TeamToolDispatcher for TeamToolbox {
    fn lost_host_records(
        &self,
        team: Uuid,
        order: Uuid,
        run: Uuid,
    ) -> BoxFuture<'_, Result<Vec<cowork_storage::team_service::TeamLostHostRecord>>> {
        Box::pin(async move {
            let c = self.containers.clone().ok_or(Error::Forbidden)?;
            tokio::task::spawn_blocking(move || c.lost_records(team, order, run))
                .await
                .map_err(|_| Error::UncertainEffect)?
        })
    }
    fn close_lost_host_record(
        &self,
        permit: cowork_storage::team_service::LostHostClosurePermit,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.containers
                .as_ref()
                .ok_or(Error::Forbidden)?
                .close_lost_record(permit)
                .await
        })
    }
    fn container_recovery_plans(
        &self,
        team: Uuid,
        order: Uuid,
        run: Uuid,
    ) -> BoxFuture<'_, Result<Vec<cowork_contracts::TeamContainerRecoveryPlan>>> {
        Box::pin(async move {
            let containers = self.containers.clone().ok_or(Error::Forbidden)?;
            tokio::task::spawn_blocking(move || containers.recovery_plans(team, order, run))
                .await
                .map_err(|_| Error::UncertainEffect)?
        })
    }
    fn retire_container(
        &self,
        permit: cowork_storage::team_service::ContainerRecoveryPermit,
    ) -> BoxFuture<'_, Result<cowork_storage::team_service::ContainerClosedEvidence>> {
        Box::pin(async move {
            self.containers
                .as_ref()
                .ok_or(Error::Forbidden)?
                .retire_original(permit)
                .await
        })
    }
    fn close_container_record(
        &self,
        permit: cowork_storage::team_service::ContainerRecoveryPermit,
        receipt: cowork_contracts::TeamContainerRecoveryReceipt,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.containers
                .as_ref()
                .ok_or(Error::Forbidden)?
                .close_original_record(permit, receipt)
                .await
        })
    }
    fn definitions(&self, names: &[String]) -> Result<Vec<Value>> {
        names.iter().map(|name| self.definition(name)).collect()
    }
    fn execute(
        &self,
        authority: TeamExecutionAuthority,
        name: String,
        arguments: Value,
    ) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move { self.perform(authority, name, arguments).await })
    }
}
