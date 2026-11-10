//! Original private GitHub effects, repository state and post-removal workspace CAS.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use crate::android_private_github as github;
use crate::android_private_server::{id, leased, row};
use crate::android_private_tools as tools;
use crate::android_private_workspace as workspace;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction, any::AnyRow};
use zeroize::Zeroizing;

pub struct GithubAdmission {
    pub session: Uuid,
    pub sequence: i64,
    pub invocation: String,
    pub request_sha256: String,
    pub action: Value,
    pub binding: Value,
    pub git_files: Value,
}
pub fn oid(raw: &str) -> bool {
    (raw.len() == 40 || raw.len() == 64)
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn branch(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 200
        && !raw.starts_with(['-', '.', '/'])
        && !raw.ends_with(['.', '/'])
        && !raw.contains(['\\', '~', '^', ':', '?', '*', '[', ' '])
        && !raw.contains("..")
        && !raw.contains("@{")
        && raw
            .split('/')
            .all(|s| !s.is_empty() && !s.starts_with('.') && !s.ends_with(".lock"))
        && !raw.chars().any(char::is_control)
}
pub fn action(tool: &ToolCall) -> Result<Value> {
    if tool.name != "github" {
        return Err(Error::Forbidden);
    }
    let v = &tool.arguments;
    let o = v.as_object().ok_or_else(device_invalid)?;
    if o.keys().any(|s| {
        !matches!(
            s.as_str(),
            "operation"
                | "session_id"
                | "directory"
                | "branch"
                | "expected_head"
                | "expected_remote"
                | "expected_tree_sha256"
                | "message"
                | "title"
                | "body"
                | "base"
                | "number"
                | "draft"
                | "merge_method"
                | "create_branch"
        )
    }) {
        return Err(device_invalid());
    }
    let op = v["operation"].as_str().ok_or_else(device_invalid)?;
    let allowed: &[&str] = match op {
        "clone" => &["operation", "directory", "branch"],
        "open" => &["operation", "directory"],
        "fetch" | "status" | "close" => &["operation", "session_id"],
        "pull" => &["operation", "session_id", "branch", "expected_head"],
        "checkout" => &[
            "operation",
            "session_id",
            "branch",
            "expected_head",
            "create_branch",
        ],
        "commit" => &[
            "operation",
            "session_id",
            "message",
            "expected_head",
            "expected_tree_sha256",
        ],
        "push" => &[
            "operation",
            "session_id",
            "branch",
            "expected_head",
            "expected_remote",
            "expected_tree_sha256",
        ],
        "pr_create" => &[
            "operation",
            "session_id",
            "branch",
            "base",
            "expected_head",
            "title",
            "body",
            "draft",
        ],
        "pr_list" | "issue_list" => &["operation", "session_id"],
        "pr_view" | "pr_close" | "issue_view" | "issue_close" => {
            &["operation", "session_id", "number"]
        }
        "pr_merge" => &[
            "operation",
            "session_id",
            "number",
            "expected_head",
            "merge_method",
        ],
        "issue_create" => &["operation", "session_id", "title", "body"],
        "issue_comment" => &["operation", "session_id", "number", "body"],
        _ => return Err(Error::Forbidden),
    };
    if o.keys().any(|s| !allowed.contains(&s.as_str())) {
        return Err(device_invalid());
    }
    for key in allowed {
        if matches!(*key, "branch" | "draft") && op == "clone" {
            continue;
        }
        if matches!(*key, "draft" | "create_branch") {
            continue;
        }
        if !o.contains_key(*key) {
            return Err(device_invalid());
        }
    }
    for key in ["branch", "base"] {
        if let Some(x) = v.get(key) {
            if !x.as_str().is_some_and(branch) {
                return Err(device_invalid());
            }
        }
    }
    for key in ["expected_head", "expected_remote"] {
        if let Some(x) = v.get(key) {
            if !x.as_str().is_some_and(oid) {
                return Err(device_invalid());
            }
        }
    }
    if matches!(op, "push" | "pr_create" | "pr_merge")
        && v["expected_head"]
            .as_str()
            .is_some_and(|s| s.bytes().all(|b| b == b'0'))
    {
        return Err(device_invalid());
    }
    if let Some(x) = v.get("expected_tree_sha256") {
        if !x.as_str().is_some_and(|s| s.len() == 64 && oid(s)) {
            return Err(device_invalid());
        }
    }
    if let Some(x) = v.get("directory") {
        let d = x.as_str().ok_or_else(device_invalid)?;
        crate::android_private_snapshots::relative(d)?;
        if d.split('/').any(|s| s.eq_ignore_ascii_case(".git")) {
            return Err(device_invalid());
        }
    }
    for (k, max) in [("message", 32768), ("title", 256), ("body", 65536)] {
        if let Some(x) = v.get(k) {
            let s = x.as_str().ok_or_else(device_invalid)?;
            if s.len() > max || s.contains('\0') || k != "body" && s.is_empty() {
                return Err(device_invalid());
            }
        }
    }
    if v.get("number")
        .is_some_and(|x| !x.as_u64().is_some_and(|n| n > 0 && n <= i32::MAX as u64))
        || v.get("draft").is_some_and(|x| !x.is_boolean())
        || v.get("create_branch").is_some_and(|x| !x.is_boolean())
        || v.get("merge_method")
            .is_some_and(|x| !matches!(x.as_str(), Some("merge" | "squash" | "rebase")))
    {
        return Err(device_invalid());
    }
    if !matches!(op, "clone" | "open") {
        Uuid::parse_str(v["session_id"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?;
    }
    Ok(v.clone())
}
/// Exact approved API request; no arbitrary `gh api`, host, credential or shell arguments.
pub fn api_request(binding: &Value, a: &Value, preflight: bool) -> Result<Value> {
    let root = format!(
        "{}repos/{}/{}",
        binding["api_root"].as_str().ok_or_else(device_invalid)?,
        binding["owner"].as_str().ok_or_else(device_invalid)?,
        binding["repository"].as_str().ok_or_else(device_invalid)?
    );
    let op = a["operation"].as_str().ok_or_else(device_invalid)?;
    if preflight {
        if op != "pr_create" {
            return Err(Error::Forbidden);
        }
        let mut url = reqwest::Url::parse(&root).map_err(|_| device_invalid())?;
        url.path_segments_mut()
            .map_err(|_| device_invalid())?
            .extend(["git", "ref", "heads"])
            .extend(a["branch"].as_str().ok_or_else(device_invalid)?.split('/'));
        return Ok(
            json!({"method":"GET","url":url.as_str(),"body_base64":"","channel":"api","preflight":true}),
        );
    }
    let n = a["number"].as_u64().unwrap_or(0);
    let (method, path, payload) = match op {
        "pr_create" => (
            "POST",
            "/pulls".into(),
            json!({"head":a["branch"],"base":a["base"],"title":a["title"],"body":a["body"],"draft":a["draft"].as_bool().unwrap_or(false)}),
        ),
        "pr_list" => ("GET", "/pulls?state=all&per_page=100".into(), Value::Null),
        "pr_view" => ("GET", format!("/pulls/{n}"), Value::Null),
        "pr_merge" => (
            "PUT",
            format!("/pulls/{n}/merge"),
            json!({"sha":a["expected_head"],"merge_method":a["merge_method"]}),
        ),
        "pr_close" => ("PATCH", format!("/pulls/{n}"), json!({"state":"closed"})),
        "issue_list" => ("GET", "/issues?state=all&per_page=100".into(), Value::Null),
        "issue_view" => ("GET", format!("/issues/{n}"), Value::Null),
        "issue_create" => (
            "POST",
            "/issues".into(),
            json!({"title":a["title"],"body":a["body"]}),
        ),
        "issue_comment" => (
            "POST",
            format!("/issues/{n}/comments"),
            json!({"body":a["body"]}),
        ),
        "issue_close" => ("PATCH", format!("/issues/{n}"), json!({"state":"closed"})),
        _ => return Err(Error::Forbidden),
    };
    let body = if payload.is_null() {
        String::new()
    } else {
        STANDARD.encode(json_string(&payload)?.as_bytes())
    };
    Ok(
        json!({"method":method,"url":format!("{root}{path}"),"body_base64":body,"channel":"api","preflight":false}),
    )
}
pub fn network_allowed(binding: &Value, a: &Value, n: &Value) -> Result<()> {
    if n["channel"] == "api" {
        if api_request(binding, a, n["preflight"] == true)? != *n {
            return Err(Error::Forbidden);
        }
        return Ok(());
    }
    if n["channel"] != "git" || n.as_object().is_none_or(|v| v.len() != 4) {
        return Err(Error::Forbidden);
    }
    let root = format!(
        "{}/{}/{}.git",
        binding["git_origin"].as_str().ok_or_else(device_invalid)?,
        binding["owner"].as_str().ok_or_else(device_invalid)?,
        binding["repository"].as_str().ok_or_else(device_invalid)?
    );
    let op = a["operation"].as_str().ok_or_else(device_invalid)?;
    let url = n["url"].as_str().ok_or_else(device_invalid)?;
    let method = n["method"].as_str().ok_or_else(device_invalid)?;
    let upload = matches!(op, "clone" | "fetch" | "pull" | "push");
    let receive = op == "push";
    let accepted = method == "GET"
        && ((upload && url == format!("{root}/info/refs?service=git-upload-pack"))
            || (receive && url == format!("{root}/info/refs?service=git-receive-pack")))
        || method == "POST"
            && ((upload && url == format!("{root}/git-upload-pack"))
                || (receive && url == format!("{root}/git-receive-pack")));
    if !accepted {
        return Err(Error::Forbidden);
    }
    let raw = Zeroizing::new(
        STANDARD
            .decode(n["body_base64"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?,
    );
    if raw.len() > 8 * 1024 * 1024 || method == "GET" && !raw.is_empty() {
        return Err(Error::BudgetExceeded);
    }
    if url.ends_with("/git-receive-pack") {
        // The one original command must be the approved old/new/ref. Force/delete/extra commands cannot cross this broker.
        if raw.len() < 4 {
            return Err(Error::Forbidden);
        }
        let length = usize::from_str_radix(
            std::str::from_utf8(&raw[..4]).map_err(|_| device_invalid())?,
            16,
        )
        .map_err(|_| device_invalid())?;
        if !(4..=1024).contains(&length)
            || raw.len() < length + 4
            || &raw[length..length + 4] != b"0000"
        {
            return Err(Error::Forbidden);
        }
        let command = std::str::from_utf8(&raw[4..length])
            .map_err(|_| device_invalid())?
            .split('\0')
            .next()
            .unwrap_or("")
            .trim_end_matches('\n');
        let expected = format!(
            "{} {} refs/heads/{}",
            a["expected_remote"].as_str().ok_or_else(device_invalid)?,
            a["expected_head"].as_str().ok_or_else(device_invalid)?,
            a["branch"].as_str().ok_or_else(device_invalid)?
        );
        if command != expected {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
async fn original(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    sequence: i64,
    keys: &dyn KeyStore,
) -> Result<Value> {
    let raw:String=sqlx::query_scalar("SELECT intent FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND kind='process'").bind(id(r,"id")?.to_string()).bind(sequence).fetch_one(&mut **tx).await.map_err(storage_error)?;
    serde_json::from_slice(&workspace::open_value(
        keys,
        r,
        &format!("intent:{sequence}"),
        &parse(&raw)?,
    )?)
    .map_err(storage_error)
}
async fn live(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    lease: Uuid,
    session: Uuid,
    sequence: i64,
    keys: &dyn KeyStore,
) -> Result<(AnyRow, Value, Value)> {
    let now = device_writer(tx).await?;
    crate::server_keys::maintenance_fence(tx).await?;
    let r = leased(tx, run, lease, now).await?;
    let cp = tools::checkpoint(tx, &r, keys).await?;
    tools::loop_authority(tx, &r, &cp, now).await?;
    let s=sqlx::query("SELECT * FROM android_private_github_sessions WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='admitted' AND last_sequence=$4").bind(session.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(sequence).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if s.try_get::<String, _>("owner_id").map_err(storage_error)? != id(&r, "owner_id")?.to_string()
        || s.try_get::<String, _>("project_id")
            .map_err(storage_error)?
            != id(&r, "project_id")?.to_string()
    {
        return Err(Error::Forbidden);
    }
    let intent = original(tx, &r, sequence, keys).await?;
    let tool = tools::pending(&cp)?;
    let original_run: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let frozen = github::validate_binding(
        tx,
        id(&r, "owner_id")?,
        id(&r, "project_id")?,
        &original_run["spec"]["input"],
    )
    .await?
    .ok_or(Error::Forbidden)?;
    let step=sqlx::query("SELECT checkpoint_sha,request_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='process'").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if intent["tool"] != json!(tool)
        || intent["fingerprint"] != tools::tool_fingerprint(run, &cp, &tool)
        || intent["binding"] != frozen
        || frozen["id"]
            != s.try_get::<String, _>("binding_id")
                .map_err(storage_error)?
        || frozen["binding_sha256"]
            != s.try_get::<String, _>("binding_sha")
                .map_err(storage_error)?
        || intent["action"]["directory"]
            != s.try_get::<String, _>("directory").map_err(storage_error)?
        || intent["action"]["session_id"] != session.to_string()
        || canonical_hash(&intent)
            != step
                .try_get::<String, _>("request_sha")
                .map_err(storage_error)?
        || canonical_hash(&cp)
            != step
                .try_get::<String, _>("checkpoint_sha")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let req = tools::request(tx, &r, &cp, &tool, "approval", now).await?;
    if req["state"] != "approved"
        || req["id"] != intent["approval_id"]
        || req["revision"] != intent["approval_revision"]
        || chrono::DateTime::parse_from_rfc3339(
            req["expires_at"].as_str().ok_or_else(device_invalid)?,
        )
        .map_err(|_| device_invalid())?
        .timestamp_millis()
            <= now
    {
        return Err(Error::Forbidden);
    }
    tools::live_after_crypto(tx, &r, &cp).await?;
    Ok((r, cp, intent))
}
/// Ready sessions contain only frozen metadata. Close them in the actual final-run transaction.
pub(crate) async fn finish(tx: &mut Transaction<'_, Any>, run: Uuid) -> Result<()> {
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_sessions s WHERE s.run_id=$1 AND (s.state IN('admitted','uncertain') OR s.state='ready' AND NOT EXISTS(SELECT 1 FROM android_private_containers c WHERE c.id=s.container_nonce AND c.run_id=s.run_id AND c.lease_id=s.lease_id AND c.state='removed' AND c.termination_witness IS NOT NULL))").bind(run.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let network:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_requests q JOIN android_private_github_sessions s ON s.id=q.session_id WHERE s.run_id=$1 AND (q.state!='received' OR q.receipt_version IS NULL)").bind(run.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 0 || network != 0 {
        return Err(Error::UncertainEffect);
    }
    sqlx::query("UPDATE android_private_github_sessions SET state='closed',revision=revision+1 WHERE run_id=$1 AND state='ready'").bind(run.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl Store {
    pub async fn android_private_github_gate(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<crate::android_private_mcp_runtime::PrivateMcpGate> {
        use crate::android_private_mcp_runtime::PrivateMcpGate;
        let cipher = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = leased(&mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(&mut tx, &r, &cp, now).await?;
        action(&tools::pending(&cp)?)?;
        let original_run: Value =
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if original_run["spec"]["input"]["frozen_private_runner"]["github"] != true
            || !cp["spec"]["allowed_tools"]
                .as_array()
                .is_some_and(|v| v.contains(&json!("github")))
        {
            return Err(Error::Forbidden);
        }
        github::validate_binding(
            &mut tx,
            id(&r, "owner_id")?,
            id(&r, "project_id")?,
            &original_run["spec"]["input"],
        )
        .await?
        .ok_or(Error::Forbidden)?;
        let tool = tools::pending(&cp)?;
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        let value = match req["state"].as_str() {
            Some("pending") => PrivateMcpGate::Waiting(req),
            Some("approved") => PrivateMcpGate::Ready,
            Some("rejected") => PrivateMcpGate::Denied(
                tools::complete_tool(
                    &mut tx,
                    &r,
                    &mut cp,
                    &tool,
                    &json!({"denied":true}),
                    keys,
                    cipher,
                    now,
                )
                .await?,
            ),
            _ => return Err(Error::Forbidden),
        };
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_private_github_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<GithubAdmission> {
        let cipher = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = leased(&mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(&mut tx, &r, &cp, now).await?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        let original_run: Value =
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let input = &original_run["spec"]["input"];
        if !spec.allowed_tools.iter().any(|s| s == "github")
            || input["frozen_private_runner"]["github"] != true
        {
            return Err(Error::Forbidden);
        }
        if cp["model_steps"].as_u64().ok_or_else(device_invalid)?
            + cp["tool_steps"].as_u64().ok_or_else(device_invalid)?
            >= u64::from(spec.budgets.max_steps)
        {
            return Err(Error::BudgetExceeded);
        }
        let binding =
            github::validate_binding(&mut tx, id(&r, "owner_id")?, id(&r, "project_id")?, input)
                .await?
                .ok_or(Error::Forbidden)?;
        let tool = tools::pending(&cp)?;
        let mut a = action(&tool)?;
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        if req["state"] != "approved"
            || chrono::DateTime::parse_from_rfc3339(
                req["expires_at"].as_str().ok_or_else(device_invalid)?,
            )
            .map_err(|_| device_invalid())?
            .timestamp_millis()
                <= now
        {
            return Err(Error::Forbidden);
        }
        let sequence = tools::next_sequence(&mut tx, run).await?;
        let baseline = workspace::entries(&mut tx, run).await?;
        let baseline_version = workspace::add_version(
            &mut tx,
            &r,
            &format!(".github/baseline/{sequence}"),
            "artifact",
            json_string(&baseline)?.as_bytes(),
            keys,
            cipher,
            now,
        )
        .await?;
        let (session, git_files) = if matches!(a["operation"].as_str(), Some("clone" | "open")) {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_sessions WHERE run_id=$1 AND state!='closed'").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count >= 4 {
                return Err(Error::BudgetExceeded);
            }
            let directory = a["directory"].as_str().unwrap();
            if baseline.iter().any(|e| {
                e["path"]
                    .as_str()
                    .is_some_and(|p| p == directory || p.starts_with(&format!("{directory}/")))
            }) {
                return Err(Error::Conflict);
            }
            let session = Uuid::new_v4();
            sqlx::query("INSERT INTO android_private_github_sessions(id,run_id,owner_id,project_id,lease_id,binding_id,binding_sha,directory,baseline_version,last_sequence,state,revision,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'admitted',1,$11)").bind(session.to_string()).bind(run.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(lease.to_string()).bind(binding["id"].as_str().unwrap()).bind(binding["binding_sha256"].as_str().unwrap()).bind(directory).bind(&baseline_version).bind(sequence).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
            (session, json!([]))
        } else {
            let session =
                Uuid::parse_str(a["session_id"].as_str().unwrap()).map_err(|_| device_invalid())?;
            let s=sqlx::query("SELECT directory,git_version,last_sequence FROM android_private_github_sessions WHERE id=$1 AND run_id=$2 AND owner_id=$3 AND project_id=$4 AND lease_id=$5 AND binding_id=$6 AND binding_sha=$7 AND state='ready'").bind(session.to_string()).bind(run.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(lease.to_string()).bind(binding["id"].as_str().unwrap()).bind(binding["binding_sha256"].as_str().unwrap()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
            a["directory"] = json!(s.try_get::<String, _>("directory").map_err(storage_error)?);
            let version: String = s.try_get("git_version").map_err(storage_error)?;
            let bytes = workspace::version_bytes(&mut tx, &r, &version, keys).await?;
            let archive: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
            let previous_sequence: i64 = s.try_get("last_sequence").map_err(storage_error)?;
            let original_receipt_sha:String=sqlx::query_scalar("SELECT receipt_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='process' AND state='received'").bind(run.to_string()).bind(previous_sequence).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if archive["format"] != 1
                || archive["session_id"] != session.to_string()
                || archive["directory"] != a["directory"]
                || archive["binding_sha256"] != binding["binding_sha256"]
                || archive["sequence"] != previous_sequence
                || archive["receipt_sha256"] != original_receipt_sha
            {
                return Err(Error::Forbidden);
            }
            let files = archive["git_files"].clone();
            sqlx::query("UPDATE android_private_github_sessions SET state='admitted',last_sequence=$1,baseline_version=$2,container_nonce=NULL,revision=revision+1 WHERE id=$3 AND state='ready'").bind(sequence).bind(&baseline_version).bind(session.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            (session, files)
        };
        a["session_id"] = json!(session);
        a["author_name"] = binding["author_name"].clone();
        a["author_email"] = binding["author_email"].clone();
        let intent = json!({"format":1,"tool":tool,"fingerprint":tools::tool_fingerprint(run,&cp,&tool),"runner":input["frozen_private_runner"],"action":a,"binding":binding,"baseline_version":baseline_version,"approval_id":req["id"],"approval_revision":req["revision"]});
        let sealed = workspace::seal_value(
            keys,
            cipher,
            &r,
            &format!("intent:{sequence}"),
            json_string(&intent)?.as_bytes(),
        )?;
        crate::server_keys::cipher_writer(&mut tx, cipher).await?;
        let invocation = tools::invocation(&cp, &tool)?;
        sqlx::query("INSERT INTO android_private_loop_steps(run_id,sequence,invocation_id,kind,lease_id,checkpoint_sha,request_sha,intent,state,created_at) VALUES($1,$2,$3,'process',$4,$5,$6,$7,'admitted',$8)").bind(run.to_string()).bind(sequence).bind(&invocation).bind(lease.to_string()).bind(canonical_hash(&cp)).bind(canonical_hash(&intent)).bind(json_string(&sealed)?).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_private_server::save(&mut tx, &r, "running", None, None, now).await?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(GithubAdmission {
            session,
            sequence,
            invocation,
            request_sha256: canonical_hash(&intent),
            action: a,
            binding,
            git_files,
        })
    }
    pub async fn android_private_github_attach(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (_, _, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND original_binding=$4 AND state='running'").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(json_string(&intent["runner"])?).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        if sqlx::query("UPDATE android_private_github_sessions SET container_nonce=$1 WHERE id=$2 AND last_sequence=$3 AND state='admitted' AND container_nonce IS NULL").bind(nonce.to_string()).bind(session.to_string()).bind(sequence).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_github_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (_, _, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_sessions s JOIN android_private_containers c ON c.id=s.container_nonce WHERE s.id=$1 AND c.id=$2 AND c.run_id=$3 AND c.lease_id=$4 AND c.original_binding=$5 AND c.state='running'").bind(session.to_string()).bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(json_string(&intent["runner"])?).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_github_network_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        request: Uuid,
        network: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        if request.is_nil() || json_string(network)?.len() > 12 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let cipher = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (r, cp, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        network_allowed(&intent["binding"], &intent["action"], network)?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers c JOIN android_private_github_sessions s ON s.container_nonce=c.id WHERE s.id=$1 AND c.id=$2 AND c.state='running'").bind(session.to_string()).bind(nonce.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_requests WHERE session_id=$1 AND action_sequence=$2").bind(session.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count >= 32 {
            return Err(Error::BudgetExceeded);
        }
        let effect = network["channel"] == "git"
            && network["url"]
                .as_str()
                .is_some_and(|u| u.ends_with("/git-receive-pack"))
            || network["channel"] == "api" && network["method"] != "GET";
        if effect {
            let effects:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_requests WHERE session_id=$1 AND action_sequence=$2 AND effect=1").bind(session.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if effects != 0 {
                return Err(Error::UncertainEffect);
            }
        }
        if network["channel"] == "api" {
            let expected = if intent["action"]["operation"] == "pr_create" {
                if network["preflight"] == true { 0 } else { 1 }
            } else {
                0
            };
            if count != expected {
                return Err(Error::Forbidden);
            }
            if expected == 1 {
                let version:String=sqlx::query_scalar("SELECT receipt_version FROM android_private_github_requests WHERE session_id=$1 AND action_sequence=$2 AND state='received'").bind(session.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
                let bytes = workspace::version_bytes(&mut tx, &r, &version, keys).await?;
                let evidence: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
                let body = STANDARD
                    .decode(
                        evidence["response"]["body_base64"]
                            .as_str()
                            .ok_or_else(device_invalid)?,
                    )
                    .map_err(|_| device_invalid())?;
                let response: Value =
                    serde_json::from_slice(&body).map_err(|_| device_invalid())?;
                if evidence["response"]["status"] != 200
                    || response["object"]["sha"] != intent["action"]["expected_head"]
                {
                    return Err(Error::Conflict);
                }
            }
        }
        let now = device_writer(&mut tx).await?;
        let original = json!({"format":1,"run":run,"lease":lease,"session":session,"sequence":sequence,"nonce":nonce,"action_sha256":canonical_hash(&intent),"network":network});
        let version = workspace::add_version(
            &mut tx,
            &r,
            &format!(".github/{session}/{request}/intent"),
            "artifact",
            json_string(&original)?.as_bytes(),
            keys,
            cipher,
            now,
        )
        .await?;
        sqlx::query("INSERT INTO android_private_github_requests(session_id,action_sequence,request_id,request_sha,intent_version,state,created_at,effect) VALUES($1,$2,$3,$4,$5,'admitted',$6,$7)").bind(session.to_string()).bind(sequence).bind(request.to_string()).bind(canonical_hash(&original)).bind(version).bind(now).bind(i64::from(effect)).execute(&mut *tx).await.map_err(storage_error)?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)
    }
    /// Validate the already admitted original once; return its exact server-owned credential.
    pub async fn android_private_github_network_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        request: Uuid,
        network: &Value,
        keys: &dyn KeyStore,
    ) -> Result<Zeroizing<String>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (r, cp, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        network_allowed(&intent["binding"], &intent["action"], network)?;
        let version:String=sqlx::query_scalar("SELECT q.intent_version FROM android_private_github_requests q JOIN android_private_github_sessions s ON s.id=q.session_id JOIN android_private_containers c ON c.id=s.container_nonce WHERE q.session_id=$1 AND q.action_sequence=$2 AND q.request_id=$3 AND q.state='admitted' AND q.dispatch_admitted=0 AND q.receipt_version IS NULL AND c.id=$4 AND c.run_id=$5 AND c.lease_id=$6 AND c.state='running'").bind(session.to_string()).bind(sequence).bind(request.to_string()).bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let bytes = workspace::version_bytes(&mut tx, &r, &version, keys).await?;
        let actual: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
        if actual
            != json!({"format":1,"run":run,"lease":lease,"session":session,"sequence":sequence,"nonce":nonce,"action_sha256":canonical_hash(&intent),"network":network})
        {
            return Err(Error::Forbidden);
        }
        let credential = github::credential(&mut tx, &intent["binding"], keys).await?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        if sqlx::query("UPDATE android_private_github_requests SET dispatch_admitted=1 WHERE session_id=$1 AND request_id=$2 AND state='admitted' AND dispatch_admitted=0 AND receipt_version IS NULL").bind(session.to_string()).bind(request.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::UncertainEffect)}
        tx.commit().await.map_err(storage_error)?;
        Ok(credential)
    }
    /// Original receipt retention has no submit permission and survives parent/authority loss.
    pub async fn android_private_github_network_received(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        request: Uuid,
        network: &Value,
        response: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let cipher = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        let intent = original(&mut tx, &r, sequence, keys).await?;
        let q=sqlx::query("SELECT q.intent_version,q.request_sha,q.receipt_version FROM android_private_github_requests q JOIN android_private_github_sessions s ON s.id=q.session_id JOIN android_private_containers c ON c.id=s.container_nonce WHERE q.session_id=$1 AND q.action_sequence=$2 AND q.request_id=$3 AND s.run_id=$4 AND s.lease_id=$5 AND s.owner_id=$6 AND s.project_id=$7 AND c.id=$8 AND c.original_binding=$9 AND q.state IN('admitted','uncertain') AND q.receipt_version IS NULL").bind(session.to_string()).bind(sequence).bind(request.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(nonce.to_string()).bind(json_string(&intent["runner"])?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let version: String = q.try_get("intent_version").map_err(storage_error)?;
        let bytes = workspace::version_bytes(&mut tx, &r, &version, keys).await?;
        let actual: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
        let expected = json!({"format":1,"run":run,"lease":lease,"session":session,"sequence":sequence,"nonce":nonce,"action_sha256":canonical_hash(&intent),"network":network});
        if actual != expected
            || canonical_hash(&actual)
                != q.try_get::<String, _>("request_sha")
                    .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let evidence = json!({"format":1,"original":actual,"response":response});
        let version = workspace::add_version(
            &mut tx,
            &r,
            &format!(".github/{session}/{request}/receipt"),
            "artifact",
            json_string(&evidence)?.as_bytes(),
            keys,
            cipher,
            now,
        )
        .await?;
        if sqlx::query("UPDATE android_private_github_requests SET receipt_version=$1,state=$2 WHERE session_id=$3 AND request_id=$4 AND receipt_version IS NULL").bind(version).bind(if response["uncertain"]==true{"uncertain"}else{"received"}).bind(session.to_string()).bind(request.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        tx.commit().await.map_err(storage_error)
    }
    /// Observation of original guest control bytes, without any execution or send permission.
    /// It remains retainable after authority/lease loss and never substitutes a process receipt.
    pub async fn android_private_github_control_received(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        request_sha: &str,
        bytes: &[u8],
        observed_bytes: u64,
        observed_sha: &str,
        first_frame_complete: bool,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > 32 * 1024 * 1024 + 1
            || observed_bytes < bytes.len() as u64
            || observed_sha.len() != 64
            || !observed_sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || observed_bytes == bytes.len() as u64 && observed_sha != hash(bytes)
        {
            return Err(device_invalid());
        }
        let cipher = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        let intent = original(&mut tx, &r, sequence, keys).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_sessions s JOIN android_private_containers c ON c.id=$1 AND c.run_id=s.run_id AND c.lease_id=s.lease_id AND c.owner_id=s.owner_id AND c.project_id=s.project_id JOIN android_private_loop_steps p ON p.run_id=s.run_id AND p.sequence=s.last_sequence AND p.lease_id=s.lease_id AND p.kind='process' AND p.invocation_id=c.invocation_id WHERE s.id=$2 AND s.run_id=$3 AND s.lease_id=$4 AND s.owner_id=$5 AND s.project_id=$6 AND s.last_sequence=$7 AND s.binding_id=$8 AND s.binding_sha=$9 AND (s.container_nonce=$1 OR s.container_nonce IS NULL) AND c.original_binding=$10 AND p.request_sha=$11")
            .bind(nonce.to_string()).bind(session.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(sequence).bind(intent["binding"]["id"].as_str().ok_or_else(device_invalid)?).bind(intent["binding"]["binding_sha256"].as_str().ok_or_else(device_invalid)?).bind(json_string(&intent["runner"])?).bind(request_sha).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1
            || intent["action"]["session_id"] != session.to_string()
            || canonical_hash(&intent) != request_sha
        {
            return Err(Error::Forbidden);
        }
        // Base64 and envelope must fit the existing version codec. A retained prefix is explicit.
        let retained = &bytes[..bytes.len().min(24 * 1024 * 1024 - 64 * 1024)];
        let observation = json!({"format":1,"run_id":run,"lease_id":lease,"owner_user_id":id(&r,"owner_id")?,"project_id":id(&r,"project_id")?,"session_id":session,"action_sequence":sequence,"container_nonce":nonce,"request_sha256":request_sha,"observed_bytes":observed_bytes,"observed_sha256":observed_sha,"retained_buffer_sha256":hash(bytes),"retained_bytes":retained.len(),"prefix_only":retained.len() as u64!=observed_bytes,"first_frame_complete":first_frame_complete,"data":STANDARD.encode(retained),"uncertain":true,"execution_adopted":false,"receipt_substituted":false});
        let raw = json_string(&observation)?;
        let digest = hash(raw.as_bytes());
        let path = format!(".github/{session}/control/{sequence}/{digest}");
        let previous:Option<String>=sqlx::query_scalar("SELECT digest FROM android_private_loop_versions WHERE run_id=$1 AND path=$2 AND kind='artifact'").bind(run.to_string()).bind(&path).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(previous) = previous {
            if previous != digest {
                return Err(Error::Conflict);
            }
        } else {
            let version = workspace::add_version(
                &mut tx,
                &r,
                &path,
                "artifact",
                raw.as_bytes(),
                keys,
                cipher,
                now,
            )
            .await?;
            crate::android_private_server::event(&mut tx,run,"github_original_control_received",&json!({"session_id":session,"action_sequence":sequence,"container_nonce":nonce,"version_id":version,"observed_bytes":observed_bytes,"first_frame_complete":first_frame_complete,"prefix_only":retained.len() as u64!=observed_bytes,"physical_closure_pending":true}),now).await?;
            workspace::bounds(&mut tx, run).await?;
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_github_completed(
        &self,
        run: Uuid,
        lease: Uuid,
        permit: &GithubAdmission,
        nonce: Uuid,
        received: &Value,
        witness: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        self.android_private_program_retain_received(
            run,
            lease,
            permit.sequence,
            &permit.request_sha256,
            nonce,
            received,
            keys,
            key_id,
        )
        .await?;
        let cipher = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (r, mut cp, intent) =
            live(&mut tx, run, lease, permit.session, permit.sequence, keys).await?;
        let now = device_writer(&mut tx).await?;
        let previous = canonical_hash(&cp);
        let stopped:String=sqlx::query_scalar("SELECT termination_witness FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='removed' AND original_binding=$4").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(json_string(&intent["runner"])?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        if parse::<Value>(&stopped)? != *witness
            || received["nonce"] != nonce.to_string()
            || received["session_id"] != permit.session.to_string()
            || received["action_sequence"] != permit.sequence
            || received["action"] != intent["action"]
            || received["result"]["operation"] != intent["action"]["operation"]
        {
            return Err(Error::Forbidden);
        }
        let baseline_version = intent["baseline_version"]
            .as_str()
            .ok_or_else(device_invalid)?;
        let baseline: Value = serde_json::from_slice(
            &workspace::version_bytes(&mut tx, &r, baseline_version, keys).await?,
        )
        .map_err(storage_error)?;
        if baseline != json!(workspace::entries(&mut tx, run).await?) {
            return Err(Error::Conflict);
        }
        let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_github_requests WHERE session_id=$1 AND action_sequence=$2 AND (state!='received' OR receipt_version IS NULL)").bind(permit.session.to_string()).bind(permit.sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if unknown != 0 || received["result"]["uncertain"] == true {
            return Err(Error::UncertainEffect);
        }
        let git_files = received["git_files"]
            .as_array()
            .filter(|v| v.len() <= 10000)
            .ok_or_else(device_invalid)?;
        let directory = intent["action"]["directory"]
            .as_str()
            .ok_or_else(device_invalid)?;
        let mut total = 0usize;
        let mut names = std::collections::BTreeSet::new();
        for f in git_files {
            let p = f["path"].as_str().ok_or_else(device_invalid)?;
            crate::android_private_snapshots::relative(p)?;
            if !names.insert(p.to_ascii_lowercase())
                || p.split('/').any(|s| s.eq_ignore_ascii_case(".git"))
                || !matches!(f["kind"].as_str(), Some("file" | "directory"))
            {
                return Err(device_invalid());
            }
            if f["kind"] == "file" {
                let raw = STANDARD
                    .decode(f["data"].as_str().ok_or_else(device_invalid)?)
                    .map_err(|_| device_invalid())?;
                total = total.saturating_add(raw.len());
            }
        }
        if total > 8 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let files = received["files"].as_array().ok_or_else(device_invalid)?;
        let original_files = baseline.as_array().ok_or_else(device_invalid)?;
        for f in files {
            let p = f["path"].as_str().ok_or_else(device_invalid)?;
            if p.split('/').any(|s| s.eq_ignore_ascii_case(".git")) {
                return Err(Error::Forbidden);
            }
            if p != directory && !p.starts_with(&format!("{directory}/")) {
                if let Some(before) = original_files.iter().find(|e| e["path"] == p) {
                    if before["kind"] != f["kind"] {
                        return Err(Error::Forbidden);
                    }
                    if f["kind"] == "file" {
                        let raw = Zeroizing::new(
                            STANDARD
                                .decode(f["data"].as_str().ok_or_else(device_invalid)?)
                                .map_err(|_| device_invalid())?,
                        );
                        if before["sha256"] != hash(&raw) {
                            return Err(Error::Forbidden);
                        }
                    }
                } else if !(matches!(
                    intent["action"]["operation"].as_str(),
                    Some("clone" | "open")
                ) && f["kind"] == "directory"
                    && directory.starts_with(&format!("{p}/")))
                {
                    return Err(Error::Forbidden);
                }
            }
        }
        if original_files.iter().any(|e| {
            e["path"].as_str().is_some_and(|p| {
                p != directory
                    && !p.starts_with(&format!("{directory}/"))
                    && !files.iter().any(|f| f["path"] == p)
            })
        }) {
            return Err(Error::Forbidden);
        }
        if received["result"]["exit_code"] == 0 {
            if intent["action"]["operation"] != "close"
                && (git_files.is_empty()
                    || !files
                        .iter()
                        .any(|f| f["path"] == directory && f["kind"] == "directory"))
            {
                return Err(Error::Forbidden);
            }
            crate::android_private_runner::promote(
                &mut tx,
                &r,
                &received["files"],
                keys,
                cipher,
                now,
            )
            .await?;
            let version = workspace::add_version(
                &mut tx,
                &r,
                &format!(".github/{}/metadata/{}", permit.session, permit.sequence),
                "artifact",
                json_string(&json!({"format":1,"session_id":permit.session,"directory":directory,"binding_sha256":intent["binding"]["binding_sha256"],"sequence":permit.sequence,"receipt_sha256":canonical_hash(received),"git_files":received["git_files"]}))?.as_bytes(),
                keys,
                cipher,
                now,
            )
            .await?;
            if sqlx::query("UPDATE android_private_github_sessions SET git_version=$1,state=$2,revision=revision+1 WHERE id=$3 AND last_sequence=$4 AND container_nonce=$5 AND state='admitted'").bind(version).bind(if intent["action"]["operation"]=="close"{"closed"}else{"ready"}).bind(permit.session.to_string()).bind(permit.sequence).bind(nonce.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        } else {
            // Failed/partially applied local operations are observed once, never promoted or silently retried.
            sqlx::query("UPDATE android_private_github_sessions SET state='closed',revision=revision+1 WHERE id=$1 AND state='admitted'").bind(permit.session.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let tool = tools::pending(&cp)?;
        let mut summary = received["result"].clone();
        summary["session_id"] = json!(permit.session);
        cp["pending_tools"]
            .as_array_mut()
            .ok_or_else(device_invalid)?
            .remove(0);
        cp["tool_steps"] = json!(cp["tool_steps"].as_u64().ok_or_else(device_invalid)? + 1);
        cp["messages"].as_array_mut().ok_or_else(device_invalid)?.push(json!({"role":"tool","tool_call_id":tool.id,"name":tool.name,"content":json_string(&summary)?}));
        tools::set_checkpoint(&mut tx, &r, &cp, Some(&previous), keys, cipher, now).await?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_github_recover(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        // Metadata sessions are not transferable to another lease, and lost external effects are never resubmitted.
        sqlx::query("UPDATE android_private_github_sessions SET state='closed',revision=revision+1 WHERE state IN('admitted','uncertain') AND container_nonce IN(SELECT id FROM android_private_containers WHERE state='removed' AND termination_witness IS NOT NULL)").execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE android_private_github_requests SET state='uncertain' WHERE state='admitted' AND session_id IN(SELECT id FROM android_private_github_sessions WHERE state='closed')").execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE android_private_github_sessions SET state='closed',revision=revision+1 WHERE state='ready' AND run_id IN(SELECT r.id FROM android_private_server_runs r WHERE r.lease_until<=$1 AND r.state NOT IN('queued','running','waiting_for_approval','waiting_for_input') AND NOT EXISTS(SELECT 1 FROM android_private_containers c WHERE c.run_id=r.id AND (c.state!='removed' OR c.termination_witness IS NULL)))").bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
}
