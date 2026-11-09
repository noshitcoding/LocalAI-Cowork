//! Installation-owner and actor-private libraries. References are content, never grants.
use super::*;
use cowork_identity::{ContentAction, Principal};
use serde::de::DeserializeOwned;
use sqlx::{Any, Transaction};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub enum LibraryOwner<'a> {
    Local,
    Actor(&'a Principal),
}
impl LibraryOwner<'_> {
    fn scope(self) -> String {
        self.namespace()
    }
    pub fn namespace(self) -> String {
        match self {
            Self::Local => "installation-owner".into(),
            Self::Actor(a) => format!("actor:{}", a.identity_id()),
        }
    }
}
fn invalid() -> Error {
    Error::Validation("Invalid library configuration".into())
}
fn text(value: &str, max: usize, required: bool) -> Result<()> {
    if value.len() > max || value.contains('\0') || (required && value.trim().is_empty()) {
        return Err(invalid());
    }
    Ok(())
}
fn name(value: &str) -> Result<()> {
    text(value, 200, true)?;
    if value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
fn identifier(value: &str) -> Result<()> {
    text(value, 200, true)?;
    if value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
pub fn validate_generation_settings(v: &GenerationSettings) -> Result<()> {
    if v.temperature
        .is_some_and(|n| !n.is_finite() || !(0.0..=2.0).contains(&n))
        || v.timeout_ms
            .is_some_and(|n| !(1000..=1_200_000).contains(&n))
    {
        return Err(invalid());
    }
    Ok(())
}
fn model(v: &AgentModelSelection) -> Result<()> {
    match v {
        AgentModelSelection::Inherit => Ok(()),
        AgentModelSelection::Provider {
            provider_id,
            provider_revision,
        } => {
            identifier(provider_id)?;
            if *provider_revision <= 0 {
                return Err(invalid());
            }
            Ok(())
        }
        AgentModelSelection::Codex {
            account_id,
            model,
            reasoning_effort,
        } => {
            identifier(account_id)?;
            if let Some(m) = model {
                identifier(m)?;
            }
            if let Some(r) = reasoning_effort {
                identifier(r)?;
            }
            Ok(())
        }
    }
}
fn source(v: &Option<LibrarySource>) -> Result<()> {
    if let Some(v) = v {
        identifier(&v.id)?;
        if v.revision <= 0 {
            return Err(invalid());
        }
    }
    Ok(())
}
fn tool_filter(value: &Option<Vec<String>>) -> Result<()> {
    if let Some(values) = value {
        if values.len() > 128 {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        for value in values {
            identifier(value)?;
            if !seen.insert(value) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
pub fn validate_effective_agent_config(v: &EffectiveAgentConfig) -> Result<()> {
    cowork_domain::validate_agent_values(v)?;
    tool_filter(&v.tool_filter)?;
    tool_filter(&v.skill_ids)?;
    tool_filter(&v.memory_ids)?;
    text(&v.instructions, 256_000, false)?;
    validate_generation_settings(&v.generation)?;
    source(&v.personality_source)?;
    source(&v.agent_source)
}
pub fn validate_agent_configuration(v: &AgentConfiguration) -> Result<()> {
    tool_filter(&v.tool_filter)?;
    for value in [&v.role, &v.goal] {
        text(value, 8000, false)?;
    }
    for value in [&v.instructions, &v.skills_markdown, &v.backstory] {
        text(value, 64_000, false)?;
    }
    validate_generation_settings(&v.generation)?;
    model(&v.model)?;
    validate_budgets(&v.budgets)?;
    for ids in [&v.native_tools, &v.skill_ids, &v.memory_ids] {
        if ids.len() > 128 {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        for id in ids {
            identifier(id)?;
            if !seen.insert(id) {
                return Err(invalid());
            }
        }
    }
    if v.mcp_tools.len() > 128 {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    for tool in &v.mcp_tools {
        identifier(&tool.server_id)?;
        identifier(&tool.tool_name)?;
        if tool.server_revision <= 0 || !seen.insert((&tool.server_id, &tool.tool_name)) {
            return Err(invalid());
        }
    }
    source(&v.personality_source)?;
    source(&v.agent_source)
}
pub fn validate_personality(v: &PersonalityDefinition) -> Result<()> {
    name(&v.name)?;
    text(&v.description, 8000, false)?;
    if let Some(icon) = &v.icon {
        text(icon, 200, false)?;
    }
    for value in [&v.role, &v.goal] {
        text(value, 8000, false)?;
    }
    for value in [&v.system_prompt, &v.skills_markdown] {
        text(value, 64_000, false)?;
    }
    validate_generation_settings(&v.generation)?;
    model(&v.model)
}
pub fn validate_preferences(v: &ApplicationPreferences) -> Result<()> {
    if v.provider_defaults.len() > 32 {
        return Err(invalid());
    }
    for (provider, id) in &v.provider_defaults {
        identifier(provider)?;
        identifier(id)?;
    }
    for id in [
        &v.default_provider_id,
        &v.default_agent_id,
        &v.default_personality_id,
    ]
    .into_iter()
    .flatten()
    {
        identifier(id)?;
    }
    if !["de", "en"].contains(&v.language.as_str())
        || !["light", "dark", "system"].contains(&v.theme.as_str())
        || !["last", "work", "settings"].contains(&v.default_start_view.as_str())
        || !["runtime", "scrollback", "restore-tabs"]
            .contains(&v.terminal_persistence_mode.as_str())
        || !(50..=200).contains(&v.font_scale)
        || !(1..=4096).contains(&v.max_tool_calls_per_loop)
        || v.chat_retention_days > 36500
        || !(1..=8760).contains(&v.db_backup_interval_hours)
    {
        return Err(invalid());
    }
    text(&v.system_prompt, 64_000, false)?;
    text(&v.command_whitelist, 32_000, false)?;
    text(&v.command_blacklist, 32_000, false)?;
    text(&v.workspace_default_path, 8192, false)?;
    validate_budgets(&v.default_budgets)
}
pub(crate) async fn permit(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    owner: LibraryOwner<'_>,
    action: ContentAction,
) -> Result<()> {
    if let LibraryOwner::Actor(actor) = owner {
        store
            .identity()
            .require_global_session_in_transaction(tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        store
            .identity()
            .require_session_action_in_transaction(tx, actor, action)
            .await
            .map_err(team_service::identity_error)?;
    }
    Ok(())
}
async fn lock(tx: &mut Transaction<'_, Any>) -> Result<()> {
    sqlx::query("UPDATE library_config_guard SET id=id WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(())
}
async fn original(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    key: &str,
    digest: &str,
) -> Result<Option<Value>> {
    let row=sqlx::query("SELECT request_sha256,receipt FROM library_config_operations WHERE scope=$1 AND operation_key=$2")
        .bind(scope).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if let Some(row) = row {
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != digest
        {
            return Err(Error::Conflict);
        }
        return Ok(Some(parse(
            &row.try_get::<String, _>("receipt").map_err(storage_error)?,
        )?));
    }
    Ok(None)
}
async fn receipt(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    key: &str,
    digest: &str,
    value: &Value,
) -> Result<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM library_config_operations WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count >= 20000 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO library_config_operations(scope,operation_key,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4,$5)")
        .bind(scope).bind(key).bind(digest).bind(json_string(value)?).bind(now()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn record(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    kind: &str,
    id: &str,
    value: &Value,
    deleted: bool,
) -> Result<()> {
    let revision = value["revision"].as_i64().ok_or_else(invalid)?;
    let payload = json_string(value)?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM library_config_versions WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count >= 10000 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO library_config_versions(scope,kind,id,revision,payload) VALUES($1,$2,$3,$4,$5)")
        .bind(scope).bind(kind).bind(id).bind(revision).bind(&payload).execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("INSERT INTO library_config_entries(scope,kind,id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(scope,kind,id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,deleted=excluded.deleted")
        .bind(scope).bind(kind).bind(id).bind(revision).bind(payload).bind(i64::from(deleted)).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
fn key(value: &str) -> Result<()> {
    identifier(value)
}
fn uuid(value: &str) -> Result<()> {
    Uuid::parse_str(value).map_err(|_| invalid())?;
    Ok(())
}

async fn preferences_in(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<PreferencesRecord> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT payload FROM library_config_preferences WHERE scope=$1")
            .bind(scope)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    raw.map(|raw| parse(&raw)).transpose().map(|value| {
        value.unwrap_or(PreferencesRecord {
            revision: 0,
            updated_at: 0,
            value: ApplicationPreferences::default(),
        })
    })
}

async fn save_preferences_in(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    preferences: &PreferencesRecord,
) -> Result<()> {
    sqlx::query("INSERT INTO library_config_preferences(scope,revision,payload) VALUES($1,$2,$3) ON CONFLICT(scope) DO UPDATE SET revision=excluded.revision,payload=excluded.payload")
        .bind(scope).bind(preferences.revision).bind(json_string(preferences)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

/// The preference reference and the legacy personality marker are one choice.
/// Every changed marker is a new immutable personality revision, so concurrent
/// editors observe a CAS conflict rather than overwriting that choice.
async fn sync_personality_defaults(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    selected: Option<&str>,
    skip: Option<&str>,
    stamp: i64,
) -> Result<()> {
    if let Some(id) = selected.filter(|id| Some(*id) != skip) {
        let exists:i64=sqlx::query_scalar("SELECT COUNT(*) FROM library_config_entries WHERE scope=$1 AND kind='personality' AND id=$2 AND deleted=0")
            .bind(scope).bind(id).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if exists != 1 {
            return Err(Error::NotFound);
        }
    }
    let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM library_config_entries WHERE scope=$1 AND kind='personality' AND deleted=0 ORDER BY id")
        .bind(scope).fetch_all(&mut **tx).await.map_err(storage_error)?;
    for raw in rows {
        let mut personality: Value = parse(&raw)?;
        let id = personality["id"].as_str().ok_or_else(invalid)?.to_owned();
        if skip == Some(id.as_str()) {
            continue;
        }
        let desired = selected == Some(id.as_str());
        if personality["value"]["is_default"] == json!(desired) {
            continue;
        }
        personality["value"]["is_default"] = json!(desired);
        personality["revision"] = json!(
            personality["revision"]
                .as_i64()
                .ok_or_else(invalid)?
                .checked_add(1)
                .ok_or(Error::Conflict)?
        );
        personality["updated_at"] = json!(stamp);
        record(tx, scope, "personality", &id, &personality, false).await?;
    }
    Ok(())
}

impl Store {
    pub(crate) async fn library_list<T: DeserializeOwned>(
        &self,
        owner: LibraryOwner<'_>,
        kind: &str,
    ) -> Result<Vec<T>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM library_config_entries WHERE scope=$1 AND kind=$2 AND deleted=0 ORDER BY id LIMIT 1001")
            .bind(owner.scope()).bind(kind).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let values = rows.iter().map(|s| parse(s)).collect::<Result<Vec<T>>>()?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub(crate) async fn library_read<T: DeserializeOwned>(
        &self,
        owner: LibraryOwner<'_>,
        kind: &str,
        id: &str,
    ) -> Result<T> {
        uuid(id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        let payload:Option<String>=sqlx::query_scalar("SELECT payload FROM library_config_entries WHERE scope=$1 AND kind=$2 AND id=$3 AND deleted=0")
            .bind(owner.scope()).bind(kind).bind(id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let result = parse(&payload.ok_or(Error::NotFound)?)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn library_personalities(&self, owner: LibraryOwner<'_>) -> Result<Vec<Personality>> {
        self.library_list(owner, "personality").await
    }
    pub async fn library_agent_templates(
        &self,
        owner: LibraryOwner<'_>,
    ) -> Result<Vec<AgentTemplate>> {
        self.library_list(owner, "agent_template").await
    }
    pub async fn library_personality(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
    ) -> Result<Personality> {
        self.library_read(owner, "personality", id).await
    }
    pub async fn library_agent_template(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
    ) -> Result<AgentTemplate> {
        self.library_read(owner, "agent_template", id).await
    }
    pub(crate) async fn library_write(
        &self,
        owner: LibraryOwner<'_>,
        kind: &str,
        target: Option<&str>,
        operation: &str,
        expected: i64,
        value: Value,
    ) -> Result<Value> {
        key(operation)?;
        if let Some(id) = target {
            uuid(id)?;
        }
        if expected < 0
            || target.is_none() && expected != 0
            || json_string(&value)?.len() > 256 * 1024
        {
            return Err(invalid());
        }
        let digest = canonical_hash(
            &json!({"kind":kind,"target":target,"expected_revision":expected,"value":value}),
        );
        let scope = owner.scope();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        lock(&mut tx).await?;
        if let Some(result) = original(&mut tx, &scope, operation, &digest).await? {
            permit(self, &mut tx, owner, ContentAction::Write).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let id = target.map(str::to_owned).unwrap_or_else(crate::id);
        let stamp = now();
        let old:Option<String>=sqlx::query_scalar("SELECT payload FROM library_config_entries WHERE scope=$1 AND kind=$2 AND id=$3 AND deleted=0")
            .bind(&scope).bind(kind).bind(&id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if target.is_some() && old.is_none() {
            return Err(Error::NotFound);
        }
        let old: Option<Value> = old.map(|s| parse(&s)).transpose()?;
        if old
            .as_ref()
            .map_or(0, |v| v["revision"].as_i64().unwrap_or(-1))
            != expected
        {
            return Err(Error::Conflict);
        }
        if target.is_none() {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM library_config_entries WHERE scope=$1 AND kind=$2 AND deleted=0").bind(&scope).bind(kind).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count >= 1000 {
                return Err(Error::BudgetExceeded);
            }
        }
        if kind == "agent_template" && !value["personality"].is_null() {
            let p: Personality =
                serde_json::from_value(value["personality"].clone()).map_err(|_| invalid())?;
            let saved:Option<String>=sqlx::query_scalar("SELECT payload FROM library_config_versions WHERE scope=$1 AND kind='personality' AND id=$2 AND revision=$3")
                .bind(&scope).bind(&p.id).bind(p.revision).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            if saved.map(|s| parse::<Value>(&s)).transpose()?.as_ref()
                != Some(&value["personality"])
            {
                return Err(Error::Conflict);
            }
        }
        if kind == "personality" {
            let mut preferences = preferences_in(&mut tx, &scope).await?;
            let selected = if value["is_default"] == true {
                Some(id.clone())
            } else if preferences.value.default_personality_id.as_deref() == Some(id.as_str()) {
                None
            } else {
                preferences.value.default_personality_id.clone()
            };
            sync_personality_defaults(&mut tx, &scope, selected.as_deref(), Some(&id), stamp)
                .await?;
            if preferences.value.default_personality_id != selected {
                preferences.value.default_personality_id = selected;
                preferences.revision =
                    preferences.revision.checked_add(1).ok_or(Error::Conflict)?;
                preferences.updated_at = stamp;
                save_preferences_in(&mut tx, &scope, &preferences).await?;
            }
        }
        let revision = expected.checked_add(1).ok_or(Error::Conflict)?;
        let result = json!({"id":id,"revision":revision,"created_at":old.as_ref().map_or(stamp,|v|v["created_at"].as_i64().unwrap_or(stamp)),"updated_at":stamp,"value":value});
        record(&mut tx, &scope, kind, &id, &result, false).await?;
        receipt(&mut tx, &scope, operation, &digest, &result).await?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn library_write_personality(
        &self,
        owner: LibraryOwner<'_>,
        id: Option<&str>,
        v: &WritePersonality,
    ) -> Result<Personality> {
        validate_personality(&v.value)?;
        serde_json::from_value(
            self.library_write(
                owner,
                "personality",
                id,
                &v.idempotency_key,
                v.expected_revision,
                serde_json::to_value(&v.value).map_err(storage_error)?,
            )
            .await?,
        )
        .map_err(storage_error)
    }
    pub async fn library_write_agent_template(
        &self,
        owner: LibraryOwner<'_>,
        id: Option<&str>,
        v: &WriteAgentTemplate,
    ) -> Result<AgentTemplate> {
        name(&v.value.name)?;
        text(&v.value.description, 8000, false)?;
        if let Some(icon) = &v.value.icon {
            text(icon, 200, false)?;
        }
        validate_agent_configuration(&v.value.configuration)?;
        serde_json::from_value(
            self.library_write(
                owner,
                "agent_template",
                id,
                &v.idempotency_key,
                v.expected_revision,
                serde_json::to_value(&v.value).map_err(storage_error)?,
            )
            .await?,
        )
        .map_err(storage_error)
    }
    pub(crate) async fn library_delete(
        &self,
        owner: LibraryOwner<'_>,
        kind: &str,
        id: &str,
        v: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        uuid(id)?;
        key(&v.idempotency_key)?;
        if v.expected_revision <= 0 {
            return Err(invalid());
        }
        let scope = owner.scope();
        let digest = canonical_hash(&json!({"kind":kind,"delete":id,"request":v}));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        lock(&mut tx).await?;
        if let Some(result) = original(&mut tx, &scope, &v.idempotency_key, &digest).await? {
            permit(self, &mut tx, owner, ContentAction::Write).await?;
            tx.commit().await.map_err(storage_error)?;
            return serde_json::from_value(result).map_err(storage_error);
        }
        let raw:Option<String>=sqlx::query_scalar("SELECT payload FROM library_config_entries WHERE scope=$1 AND kind=$2 AND id=$3 AND deleted=0")
            .bind(&scope).bind(kind).bind(id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let mut old: Value = parse(&raw.ok_or(Error::NotFound)?)?;
        if old["revision"].as_i64() != Some(v.expected_revision) {
            return Err(Error::Conflict);
        }
        let revision = v.expected_revision.checked_add(1).ok_or(Error::Conflict)?;
        old["revision"] = json!(revision);
        old["updated_at"] = json!(now());
        record(&mut tx, &scope, kind, id, &old, true).await?;
        let preferences: Option<String> =
            sqlx::query_scalar("SELECT payload FROM library_config_preferences WHERE scope=$1")
                .bind(&scope)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if let Some(raw) = preferences.filter(|_| matches!(kind, "personality" | "agent_template"))
        {
            let mut preferences: PreferencesRecord = parse(&raw)?;
            let selected = if kind == "personality" {
                &mut preferences.value.default_personality_id
            } else {
                &mut preferences.value.default_agent_id
            };
            if selected.as_deref() == Some(id) {
                *selected = None;
                preferences.revision =
                    preferences.revision.checked_add(1).ok_or(Error::Conflict)?;
                preferences.updated_at = now();
                sqlx::query(
                    "UPDATE library_config_preferences SET revision=$1,payload=$2 WHERE scope=$3",
                )
                .bind(preferences.revision)
                .bind(json_string(&preferences)?)
                .bind(&scope)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            }
        }
        let result = LibraryDeleted {
            id: id.into(),
            revision,
            deleted: true,
        };
        receipt(
            &mut tx,
            &scope,
            &v.idempotency_key,
            &digest,
            &serde_json::to_value(&result).map_err(storage_error)?,
        )
        .await?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn library_delete_personality(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        v: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        self.library_delete(owner, "personality", id, v).await
    }
    pub async fn library_delete_agent_template(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        v: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        self.library_delete(owner, "agent_template", id, v).await
    }
    pub async fn library_preferences(&self, owner: LibraryOwner<'_>) -> Result<PreferencesRecord> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        let raw: Option<String> =
            sqlx::query_scalar("SELECT payload FROM library_config_preferences WHERE scope=$1")
                .bind(owner.scope())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        let result = raw
            .map(|s| parse(&s))
            .transpose()?
            .unwrap_or(PreferencesRecord {
                revision: 0,
                updated_at: 0,
                value: ApplicationPreferences::default(),
            });
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn library_write_preferences(
        &self,
        owner: LibraryOwner<'_>,
        v: &WritePreferences,
    ) -> Result<PreferencesRecord> {
        key(&v.idempotency_key)?;
        validate_preferences(&v.value)?;
        if v.expected_revision < 0 {
            return Err(invalid());
        }
        let scope = owner.scope();
        let digest = canonical_hash(&json!({"preferences":v}));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        lock(&mut tx).await?;
        if let Some(result) = original(&mut tx, &scope, &v.idempotency_key, &digest).await? {
            permit(self, &mut tx, owner, ContentAction::Write).await?;
            tx.commit().await.map_err(storage_error)?;
            return serde_json::from_value(result).map_err(storage_error);
        }
        let current: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM library_config_preferences WHERE scope=$1")
                .bind(&scope)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if current.unwrap_or(0) != v.expected_revision {
            return Err(Error::Conflict);
        }
        let result = PreferencesRecord {
            revision: v.expected_revision.checked_add(1).ok_or(Error::Conflict)?,
            updated_at: now(),
            value: v.value.clone(),
        };
        sync_personality_defaults(
            &mut tx,
            &scope,
            result.value.default_personality_id.as_deref(),
            None,
            result.updated_at,
        )
        .await?;
        let payload = serde_json::to_value(&result).map_err(storage_error)?;
        save_preferences_in(&mut tx, &scope, &result).await?;
        receipt(&mut tx, &scope, &v.idempotency_key, &digest, &payload).await?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
