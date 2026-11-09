//! Account metadata only. Authentication material stays in the OS keyring.
use super::*;
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CodexAccountDefinition {
    pub name: String,
    pub priority: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexAccount {
    pub id: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub value: CodexAccountDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteCodexAccount {
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub value: CodexAccountDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexRuntimeStatus {
    pub configured: bool,
    pub available: bool,
    pub version: String,
    pub protocol_schema: String,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodexLoginFlow {
    Browser,
    Device,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexLoginRequest {
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub flow: CodexLoginFlow,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexAccountAction {
    pub idempotency_key: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexLoginView {
    pub login_id: String,
    pub flow: CodexLoginFlow,
    pub authorization_url: Option<String>,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexAccountStatus {
    pub account: CodexAccount,
    pub status: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub rate_limits: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodexModel {
    pub id: String,
    pub name: String,
    pub default_reasoning_effort: Option<String>,
    pub reasoning_efforts: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterializeCodexModelProfile {
    pub idempotency_key: String,
    pub expected_account_revision: i64,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub context_tokens: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CodexProfileLocator {
    pub account_id: String,
    pub account_revision: i64,
    pub reasoning_effort: Option<String>,
}
/// Canonical metadata locator. This URI is never sent as a network destination.
pub fn codex_profile_endpoint(value: &CodexProfileLocator) -> Option<String> {
    let account = uuid::Uuid::parse_str(&value.account_id).ok()?;
    if account.is_nil()
        || account.to_string() != value.account_id
        || value.account_revision < 1
        || value.reasoning_effort.as_ref().is_some_and(|s| {
            s.is_empty()
                || s.len() > 32
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return None;
    }
    Some(format!(
        "codex://{}/{}{}",
        value.account_id,
        value.account_revision,
        value
            .reasoning_effort
            .as_ref()
            .map(|s| format!("?reasoning_effort={s}"))
            .unwrap_or_default()
    ))
}
pub fn parse_codex_profile_endpoint(raw: &str) -> Option<CodexProfileLocator> {
    let raw = raw.strip_prefix("codex://")?;
    let (address, effort) = match raw.split_once('?') {
        Some((address, query)) => (address, Some(query.strip_prefix("reasoning_effort=")?)),
        None => (raw, None),
    };
    let (account, revision) = address.split_once('/')?;
    let value = CodexProfileLocator {
        account_id: account.into(),
        account_revision: revision.parse().ok()?,
        reasoning_effort: effort.map(str::to_owned),
    };
    (codex_profile_endpoint(&value)? == format!("codex://{raw}")).then_some(value)
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CodexAccountsContract {
    pub account: CodexAccount,
    pub write: WriteCodexAccount,
    pub runtime: CodexRuntimeStatus,
    pub login_request: CodexLoginRequest,
    pub login_view: CodexLoginView,
    pub action: CodexAccountAction,
    pub status: CodexAccountStatus,
    pub model: CodexModel,
    pub materialize_model_profile: MaterializeCodexModelProfile,
}
