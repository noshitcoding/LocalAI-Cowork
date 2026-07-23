use crate::credential_store::{CredentialLocator, CredentialStore};
use reqwest::{Client, Method};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const MAX_GIT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRequest {
    pub cwd: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffRequest {
    pub cwd: String,
    pub path: Option<String>,
    #[serde(default)]
    pub staged: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPathsRequest {
    pub cwd: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitRequest {
    pub cwd: String,
    pub message: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchRequest {
    pub cwd: String,
    pub branch: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListRequest {
    pub cwd: String,
    pub state: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestNumberRequest {
    pub cwd: String,
    pub number: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCreateRequest {
    pub cwd: String,
    pub title: String,
    pub body: String,
    pub base_branch: String,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCommentRequest {
    pub cwd: String,
    pub number: u64,
    pub body: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewRequest {
    pub cwd: String,
    pub number: u64,
    pub body: String,
    pub event: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMergeRequest {
    pub cwd: String,
    pub number: u64,
    pub method: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub untracked: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryContext {
    pub root: String,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub remote_url: Option<String>,
    pub web_url: Option<String>,
    pub branch: String,
    pub default_branch: String,
    pub ahead: u64,
    pub behind: u64,
    pub dirty: bool,
    pub files: Vec<GitFileStatus>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffResponse {
    pub diff: String,
    pub truncated: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubUser {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub html_url: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubConnectionStatus {
    pub authenticated: bool,
    pub profile: Option<GitHubUser>,
    pub repository: RepositoryContext,
}

#[derive(Clone, Deserialize)]
struct ApiBranchRef {
    #[serde(rename = "ref")]
    branch_ref: String,
}

#[derive(Clone, Deserialize)]
struct ApiPullRequest {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    draft: bool,
    html_url: String,
    head: ApiBranchRef,
    base: ApiBranchRef,
    #[serde(default, deserialize_with = "deserialize_user_or_default")]
    user: GitHubUser,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    merged: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSummary {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub draft: bool,
    pub html_url: String,
    pub head_branch: String,
    pub base_branch: String,
    pub user: GitHubUser,
    pub created_at: String,
    pub updated_at: String,
    pub mergeable: Option<bool>,
    pub merged: bool,
}

impl From<ApiPullRequest> for PullRequestSummary {
    fn from(value: ApiPullRequest) -> Self {
        Self {
            number: value.number,
            title: value.title,
            body: value.body.unwrap_or_default(),
            state: value.state,
            draft: value.draft,
            html_url: value.html_url,
            head_branch: value.head.branch_ref,
            base_branch: value.base.branch_ref,
            user: value.user,
            created_at: value.created_at,
            updated_at: value.updated_at,
            mergeable: value.mergeable,
            merged: value.merged,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFile {
    pub filename: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub changes: u64,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub patch: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReview {
    pub id: u64,
    #[serde(default, deserialize_with = "deserialize_user_or_default")]
    pub user: GitHubUser,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub body: String,
    pub state: String,
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub html_url: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestComment {
    pub id: u64,
    #[serde(default, deserialize_with = "deserialize_user_or_default")]
    pub user: GitHubUser,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub html_url: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetail {
    #[serde(flatten)]
    pub pull_request: PullRequestSummary,
    pub files: Vec<PullRequestFile>,
    pub reviews: Vec<PullRequestReview>,
    pub comments: Vec<PullRequestComment>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMergeResult {
    pub sha: Option<String>,
    pub merged: bool,
    pub message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitMutationResult {
    pub message: String,
    pub repository: RepositoryContext,
}

fn deserialize_string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_user_or_default<'de, D>(deserializer: D) -> Result<GitHubUser, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<GitHubUser>::deserialize(deserializer)?.unwrap_or_default())
}

fn github_token(store: &CredentialStore) -> Result<String, String> {
    let locator = CredentialLocator {
        scope: "connector".to_string(),
        owner_id: "github".to_string(),
        field: "api_key".to_string(),
    };
    store
        .get(&locator)
        .map_err(|error| error.to_string())?
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            "Connect GitHub first. The token is read from the credential vault.".to_string()
        })
}

fn github_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("LocalAI-Cowork/0.1.8")
        .build()
        .map_err(|error| format!("Could not create the GitHub client: {error}"))
}

fn resolve_repository(cwd: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(cwd.trim());
    if cwd.trim().is_empty() {
        return Err("Choose a repository folder first.".to_string());
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|_| "The selected repository folder does not exist.".to_string())?;
    if !canonical.is_dir() {
        return Err("The selected repository path is not a folder.".to_string());
    }
    let root = run_git(&canonical, &["rev-parse", "--show-toplevel"])?;
    PathBuf::from(root.trim())
        .canonicalize()
        .map_err(|_| "Git returned an invalid repository root.".to_string())
}

fn redact_credentials(input: &str) -> String {
    let mut result = input.to_string();
    for scheme in ["https://", "http://"] {
        let mut offset = 0;
        while let Some(relative_start) = result[offset..].find(scheme) {
            let start = offset + relative_start + scheme.len();
            let boundary = result[start..]
                .find(|character: char| character.is_whitespace() || character == '/')
                .map(|value| start + value)
                .unwrap_or(result.len());
            let authority = &result[start..boundary];
            if let Some(at) = authority.rfind('@') {
                if authority[..at].contains(':') {
                    result.replace_range(start..start + at + 1, "***@");
                    offset = start + 4;
                    continue;
                }
            }
            offset = boundary;
        }
    }
    result
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(cwd).args(args);
    #[cfg(target_os = "windows")]
    crate::suppress_command_window(&mut command);
    let output = command
        .output()
        .map_err(|_| "Git is not installed or could not be started.".to_string())?;
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES || output.stderr.len() > MAX_GIT_OUTPUT_BYTES {
        return Err("Git returned more data than the app can safely process.".to_string());
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(if detail.is_empty() {
            "Git command failed.".to_string()
        } else {
            redact_credentials(detail)
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_with_paths(cwd: &Path, args: &[&str], paths: &[String]) -> Result<String, String> {
    if paths.is_empty() {
        return Err("Select at least one file.".to_string());
    }
    if paths.len() > 2_000
        || paths
            .iter()
            .any(|path| path.is_empty() || path.len() > 32_768)
    {
        return Err("The selected file list is invalid.".to_string());
    }
    let mut command = Command::new("git");
    command.arg("-C").arg(cwd).args(args).arg("--");
    for path in paths {
        command.arg(path);
    }
    #[cfg(target_os = "windows")]
    crate::suppress_command_window(&mut command);
    let output = command
        .output()
        .map_err(|_| "Git is not installed or could not be started.".to_string())?;
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES || output.stderr.len() > MAX_GIT_OUTPUT_BYTES {
        return Err("Git returned more data than the app can safely process.".to_string());
    }
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(redact_credentials(detail.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_remote(remote: &str) -> Option<(String, String)> {
    let value = remote.trim().trim_end_matches('/');
    let path = if let Some(path) = value.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = value.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = value.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = value.strip_prefix("http://github.com/") {
        path
    } else {
        value.strip_prefix("git://github.com/")?
    };
    let path = path.trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

fn parse_status(raw: &str) -> Vec<GitFileStatus> {
    let mut entries = Vec::new();
    let mut fields = raw.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 3 {
            continue;
        }
        let bytes = entry.as_bytes();
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        let path = entry[3..].to_string();
        if index == 'R' || index == 'C' {
            let _ = fields.next();
        }
        entries.push(GitFileStatus {
            path,
            index_status: index.to_string(),
            worktree_status: worktree.to_string(),
            staged: index != ' ' && index != '?',
            untracked: index == '?' && worktree == '?',
        });
    }
    entries
}

fn repository_context_from_root(root: &Path) -> Result<RepositoryContext, String> {
    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let remote = run_git(root, &["config", "--get", "remote.origin.url"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let parsed_remote = remote.as_deref().and_then(parse_remote);
    let default_branch = run_git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|value| value.trim().strip_prefix("origin/").map(str::to_string))
    .unwrap_or_else(|| "main".to_string());
    let status = run_git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let files = parse_status(&status);
    let (ahead, behind) = run_git(
        root,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    )
    .ok()
    .and_then(|value| {
        let mut parts = value.split_whitespace();
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    })
    .unwrap_or((0, 0));
    let (owner, repo) = match parsed_remote {
        Some((owner, repo)) => (Some(owner), Some(repo)),
        None => (None, None),
    };
    let web_url = match (&owner, &repo) {
        (Some(owner), Some(repo)) => Some(format!("https://github.com/{owner}/{repo}")),
        _ => None,
    };
    Ok(RepositoryContext {
        root: root.to_string_lossy().to_string(),
        owner,
        repo,
        remote_url: remote.map(|value| redact_credentials(&value)),
        web_url,
        branch,
        default_branch,
        ahead,
        behind,
        dirty: !files.is_empty(),
        files,
    })
}

fn repository_context(cwd: &str) -> Result<RepositoryContext, String> {
    let root = resolve_repository(cwd)?;
    repository_context_from_root(&root)
}

fn github_repo(cwd: &str) -> Result<(PathBuf, String, String), String> {
    let root = resolve_repository(cwd)?;
    let remote = run_git(&root, &["config", "--get", "remote.origin.url"])?;
    let (owner, repo) = parse_remote(&remote)
        .ok_or_else(|| "The origin remote is not a supported github.com repository.".to_string())?;
    Ok((root, owner, repo))
}

async fn github_api<T: DeserializeOwned>(
    client: &Client,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, String> {
    let mut request = client
        .request(method, format!("{GITHUB_API_BASE}{path}"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("GitHub could not be reached: {error}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Could not read the GitHub response: {error}"))?;
    if bytes.len() > MAX_GIT_OUTPUT_BYTES {
        return Err("GitHub returned more data than the app can safely process.".to_string());
    }
    if !status.is_success() {
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| {
                status
                    .canonical_reason()
                    .unwrap_or("request failed")
                    .to_string()
            });
        return Err(format!("GitHub API {}: {}", status.as_u16(), message));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("GitHub returned an unexpected response: {error}"))
}

fn validate_pr_text(value: &str, label: &str, required: bool) -> Result<(), String> {
    if required && value.trim().is_empty() {
        return Err(format!("{label} is required."));
    }
    if value.len() > 65_536 {
        return Err(format!("{label} is too long."));
    }
    Ok(())
}

#[tauri::command]
pub fn git_repository_status(request: RepositoryRequest) -> Result<RepositoryContext, String> {
    repository_context(&request.cwd)
}

#[tauri::command]
pub fn git_repository_diff(request: GitDiffRequest) -> Result<GitDiffResponse, String> {
    let root = resolve_repository(&request.cwd)?;
    let mut args = vec!["diff", "--no-ext-diff", "--no-color"];
    if request.staged {
        args.push("--cached");
    }
    if let Some(path) = request.path.as_deref() {
        args.push("--");
        args.push(path);
    }
    let mut diff = run_git(&root, &args)?;
    let truncated = diff.len() > MAX_DIFF_BYTES;
    if truncated {
        diff.truncate(MAX_DIFF_BYTES);
        diff.push_str("\n\n[Diff truncated by LocalAI Cowork]\n");
    }
    Ok(GitDiffResponse { diff, truncated })
}

#[tauri::command]
pub fn git_repository_stage(request: GitPathsRequest) -> Result<GitMutationResult, String> {
    let root = resolve_repository(&request.cwd)?;
    run_git_with_paths(&root, &["add"], &request.paths)?;
    Ok(GitMutationResult {
        message: format!("Staged {} file(s).", request.paths.len()),
        repository: repository_context_from_root(&root)?,
    })
}

#[tauri::command]
pub fn git_repository_unstage(request: GitPathsRequest) -> Result<GitMutationResult, String> {
    let root = resolve_repository(&request.cwd)?;
    run_git_with_paths(&root, &["reset"], &request.paths)?;
    Ok(GitMutationResult {
        message: format!("Unstaged {} file(s).", request.paths.len()),
        repository: repository_context_from_root(&root)?,
    })
}

#[tauri::command]
pub fn git_repository_commit(request: GitCommitRequest) -> Result<GitMutationResult, String> {
    let root = resolve_repository(&request.cwd)?;
    let message = request.message.trim();
    if message.is_empty() || message.len() > 16_384 {
        return Err("Enter a commit message with at most 16,384 characters.".to_string());
    }
    let output = run_git(&root, &["commit", "-m", message])?;
    Ok(GitMutationResult {
        message: output.trim().to_string(),
        repository: repository_context_from_root(&root)?,
    })
}

#[tauri::command]
pub fn git_repository_push(request: RepositoryRequest) -> Result<GitMutationResult, String> {
    let root = resolve_repository(&request.cwd)?;
    let branch = run_git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let output = run_git(&root, &["push", "--set-upstream", "origin", &branch])?;
    Ok(GitMutationResult {
        message: if output.trim().is_empty() {
            format!("Pushed {branch} to origin.")
        } else {
            output.trim().to_string()
        },
        repository: repository_context_from_root(&root)?,
    })
}

#[tauri::command]
pub fn git_repository_pull(request: RepositoryRequest) -> Result<GitMutationResult, String> {
    let root = resolve_repository(&request.cwd)?;
    let output = run_git(&root, &["pull", "--ff-only"])?;
    Ok(GitMutationResult {
        message: output.trim().to_string(),
        repository: repository_context_from_root(&root)?,
    })
}

#[tauri::command]
pub fn git_repository_create_branch(
    request: GitBranchRequest,
) -> Result<GitMutationResult, String> {
    let root = resolve_repository(&request.cwd)?;
    let branch = request.branch.trim();
    if branch.is_empty() || branch.len() > 255 {
        return Err("Enter a valid branch name.".to_string());
    }
    run_git(&root, &["check-ref-format", "--branch", branch])?;
    run_git(&root, &["switch", "-c", branch])?;
    Ok(GitMutationResult {
        message: format!("Created and switched to {branch}."),
        repository: repository_context_from_root(&root)?,
    })
}

#[tauri::command]
pub async fn github_connection_status(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: RepositoryRequest,
) -> Result<GitHubConnectionStatus, String> {
    let repository = repository_context(&request.cwd)?;
    let token = match github_token(credentials.inner()) {
        Ok(token) => token,
        Err(_) => {
            return Ok(GitHubConnectionStatus {
                authenticated: false,
                profile: None,
                repository,
            })
        }
    };
    let client = github_client()?;
    let profile = github_api::<GitHubUser>(&client, &token, Method::GET, "/user", None).await?;
    Ok(GitHubConnectionStatus {
        authenticated: true,
        profile: Some(profile),
        repository,
    })
}

#[tauri::command]
pub async fn github_list_pull_requests(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: PullRequestListRequest,
) -> Result<Vec<PullRequestSummary>, String> {
    let (_, owner, repo) = github_repo(&request.cwd)?;
    let state = match request.state.as_deref().unwrap_or("open") {
        "open" => "open",
        "closed" => "closed",
        "all" => "all",
        _ => return Err("Pull-request state must be open, closed, or all.".to_string()),
    };
    let token = github_token(credentials.inner())?;
    let client = github_client()?;
    let pulls = github_api::<Vec<ApiPullRequest>>(
        &client,
        &token,
        Method::GET,
        &format!("/repos/{owner}/{repo}/pulls?state={state}&per_page=50"),
        None,
    )
    .await?;
    Ok(pulls.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn github_get_pull_request(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: PullRequestNumberRequest,
) -> Result<PullRequestDetail, String> {
    let (_, owner, repo) = github_repo(&request.cwd)?;
    let token = github_token(credentials.inner())?;
    let client = github_client()?;
    let base_path = format!("/repos/{owner}/{repo}/pulls/{}", request.number);
    let pull_request =
        github_api::<ApiPullRequest>(&client, &token, Method::GET, &base_path, None).await?;
    let files = github_api::<Vec<PullRequestFile>>(
        &client,
        &token,
        Method::GET,
        &format!("{base_path}/files?per_page=100"),
        None,
    )
    .await?;
    let reviews = github_api::<Vec<PullRequestReview>>(
        &client,
        &token,
        Method::GET,
        &format!("{base_path}/reviews?per_page=100"),
        None,
    )
    .await?;
    let comments = github_api::<Vec<PullRequestComment>>(
        &client,
        &token,
        Method::GET,
        &format!(
            "/repos/{owner}/{repo}/issues/{}/comments?per_page=100",
            request.number
        ),
        None,
    )
    .await?;
    Ok(PullRequestDetail {
        pull_request: pull_request.into(),
        files,
        reviews,
        comments,
    })
}

#[tauri::command]
pub async fn github_create_pull_request(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: PullRequestCreateRequest,
) -> Result<PullRequestSummary, String> {
    validate_pr_text(&request.title, "Title", true)?;
    validate_pr_text(&request.body, "Body", false)?;
    let root = resolve_repository(&request.cwd)?;
    let (_, owner, repo) = github_repo(&request.cwd)?;
    let head = run_git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let base = request.base_branch.trim();
    if base.is_empty() || base.len() > 255 {
        return Err("Enter a valid base branch.".to_string());
    }
    let token = github_token(credentials.inner())?;
    let client = github_client()?;
    let pull = github_api::<ApiPullRequest>(
        &client,
        &token,
        Method::POST,
        &format!("/repos/{owner}/{repo}/pulls"),
        Some(json!({
            "title": request.title.trim(),
            "body": request.body,
            "head": head,
            "base": base,
            "draft": request.draft,
        })),
    )
    .await?;
    Ok(pull.into())
}

#[tauri::command]
pub async fn github_post_pull_request_comment(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: PullRequestCommentRequest,
) -> Result<PullRequestComment, String> {
    validate_pr_text(&request.body, "Comment", true)?;
    let (_, owner, repo) = github_repo(&request.cwd)?;
    let token = github_token(credentials.inner())?;
    let client = github_client()?;
    github_api(
        &client,
        &token,
        Method::POST,
        &format!("/repos/{owner}/{repo}/issues/{}/comments", request.number),
        Some(json!({ "body": request.body })),
    )
    .await
}

#[tauri::command]
pub async fn github_submit_pull_request_review(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: PullRequestReviewRequest,
) -> Result<PullRequestReview, String> {
    validate_pr_text(&request.body, "Review", request.event != "APPROVE")?;
    let event = match request.event.as_str() {
        "COMMENT" => "COMMENT",
        "APPROVE" => "APPROVE",
        "REQUEST_CHANGES" => "REQUEST_CHANGES",
        _ => return Err("Review event must be COMMENT, APPROVE, or REQUEST_CHANGES.".to_string()),
    };
    let (_, owner, repo) = github_repo(&request.cwd)?;
    let token = github_token(credentials.inner())?;
    let client = github_client()?;
    github_api(
        &client,
        &token,
        Method::POST,
        &format!("/repos/{owner}/{repo}/pulls/{}/reviews", request.number),
        Some(json!({ "body": request.body, "event": event })),
    )
    .await
}

#[tauri::command]
pub async fn github_merge_pull_request(
    credentials: tauri::State<'_, Arc<CredentialStore>>,
    request: PullRequestMergeRequest,
) -> Result<PullRequestMergeResult, String> {
    let method = match request.method.as_str() {
        "merge" => "merge",
        "squash" => "squash",
        "rebase" => "rebase",
        _ => return Err("Merge method must be merge, squash, or rebase.".to_string()),
    };
    let (_, owner, repo) = github_repo(&request.cwd)?;
    let token = github_token(credentials.inner())?;
    let client = github_client()?;
    github_api(
        &client,
        &token,
        Method::PUT,
        &format!("/repos/{owner}/{repo}/pulls/{}/merge", request.number),
        Some(json!({ "merge_method": method })),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{parse_remote, parse_status, redact_credentials};

    #[test]
    fn parses_supported_github_remotes() {
        assert_eq!(
            parse_remote("git@github.com:openai/codex.git"),
            Some(("openai".to_string(), "codex".to_string()))
        );
        assert_eq!(
            parse_remote("https://github.com/openai/codex.git"),
            Some(("openai".to_string(), "codex".to_string()))
        );
        assert_eq!(parse_remote("https://example.com/openai/codex.git"), None);
    }

    #[test]
    fn parses_porcelain_status() {
        let files = parse_status(" M src/main.ts\0?? notes.txt\0A  added.rs\0");
        assert_eq!(files.len(), 3);
        assert!(!files[0].staged);
        assert!(files[1].untracked);
        assert!(files[2].staged);
    }

    #[test]
    fn redacts_tokens_embedded_in_urls() {
        assert_eq!(
            redact_credentials("failed https://user:secret@github.com/a/b"),
            "failed https://***@github.com/a/b"
        );
    }
}
