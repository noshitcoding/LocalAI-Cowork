//! Structured repository requests use the same durable team tool approval as
//! remote API requests. URLs and revisions cannot smuggle Git command options.
use cowork_domain::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum GithubRequest {
    Api(Api),
    Repository(Repository),
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Api {
    arguments: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Repository {
    operation: Operation,
    repository: String,
    directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_commit: Option<String>,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Clone,
    Checkout,
    Fetch,
    Pull,
    Sync,
    Status,
    Push,
}
fn invalid() -> Error {
    Error::Validation("Invalid team GitHub repository action".into())
}
fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && ![".", ".."].contains(&value)
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && !value.starts_with(['-', '/', '.'])
        && !value.ends_with(['/', '.'])
        && !["..", "//", "@{"].iter().any(|part| value.contains(*part))
        && !value.split('/').any(|part| part.ends_with(".lock"))
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))
}
pub(super) fn definition() -> Value {
    json!({"type":"function","function":{"name":"github","description":"Request an explicitly approved GitHub API action or authenticated clone/checkout/fetch/fast-forward pull/sync/status/push in the team's isolated order workspace. Repository URLs must be HTTPS on the configured team GitHub host. Use operation, repository, directory and optional reference/depth/source_commit for repository actions; use arguments only for API actions. Checkout of an explicit revision creates a detached HEAD. Sync/pull never discard local changes or rewrite history. Push requires the exact source_commit HEAD and destination branch and never forces history. Team credentials never enter the request or repository config.","parameters":{"type":"object","oneOf":[
        {"type":"object","properties":{"arguments":{"type":"array","items":{"type":"string"}}},"required":["arguments"],"additionalProperties":false},
        {"type":"object","properties":{"operation":{"type":"string","enum":["clone","checkout","fetch","pull","sync","status","push"]},"repository":{"type":"string","description":"Exact HTTPS URL on the configured GitHub host, e.g. https://github.com/owner/repository.git"},"directory":{"type":"string","description":"Repository directory relative to this order's private workspace"},"reference":{"type":"string","description":"Branch, tag or commit; required for checkout, optional for clone/fetch/pull/sync"},"source_commit":{"type":"string","pattern":"^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$","description":"Exact committed HEAD required for push; the grant binds this commit and destination branch"},"depth":{"type":"integer","minimum":1,"maximum":1000,"description":"Optional clone depth"}},"required":["operation","repository","directory"],"additionalProperties":false}
    ]}}})
}
pub(super) fn request(value: Value, hostname: &str) -> Result<Value> {
    let mut request: GithubRequest = serde_json::from_value(value).map_err(|_| invalid())?;
    match &mut request {
        GithubRequest::Api(api) => {
            if !super::valid_github_arguments(&api.arguments) {
                return Err(invalid());
            }
        }
        GithubRequest::Repository(repo) => {
            let parsed = reqwest::Url::parse(&repo.repository).map_err(|_| invalid())?;
            let parts = parsed
                .path()
                .trim_start_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parsed.scheme() != "https"
                || parsed.host_str() != Some(hostname)
                || parsed.port().is_some()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parts.len() != 2
                || !component(parts[0])
                || !component(parts[1].strip_suffix(".git").unwrap_or(parts[1]))
                || repo.repository.contains('%')
                || repo.repository.contains('\\')
                || repo.repository.chars().any(char::is_control)
                || repo.directory.is_empty()
                || repo.directory.len() > 4096
                || repo.directory.starts_with('/')
                || repo.directory.contains(['\\', ':'])
                || repo.directory.chars().any(char::is_control)
                || repo
                    .directory
                    .split('/')
                    .any(|part| part.is_empty() || [".", "..", ".git"].contains(&part))
                || repo
                    .reference
                    .as_ref()
                    .is_some_and(|value| !reference(value))
                || (repo.operation == Operation::Checkout && repo.reference.is_none())
                || repo.depth.is_some_and(|depth| !(1..=1000).contains(&depth))
                || (repo.operation != Operation::Clone && repo.depth.is_some())
                || (repo.operation == Operation::Status && repo.reference.is_some())
                || (repo.operation == Operation::Push
                    && (repo.reference.is_none()
                        || repo.source_commit.as_ref().is_none_or(|v| {
                            ![40, 64].contains(&v.len())
                                || !v.bytes().all(|b| b.is_ascii_hexdigit())
                        })))
                || (repo.operation != Operation::Push && repo.source_commit.is_some())
            {
                return Err(invalid());
            }
            repo.repository = format!(
                "https://{hostname}/{}/{}.git",
                parts[0],
                parts[1].strip_suffix(".git").unwrap_or(parts[1])
            );
        }
    }
    serde_json::to_value(request).map_err(cowork_domain::storage_error)
}
