//! Team-owned MCP and GitHub configuration. Transport state is not authority.
//! Every call uses the original run's immutable bindings and a durable approval.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_mcp::{
    Catalog, CheckpointScope, Endpoint, Limits, SealedSession, Session, SessionCipher,
};
use zeroize::Zeroize;
#[path = "team_mcp_tasks.rs"]
mod tasks;
pub(super) use tasks::interactions::with_mcp_sampling_context;
#[path = "team_mcp_oauth.rs"]
mod oauth;

pub(super) async fn migrate(tx: &mut Transaction<'_, Any>) -> Result<()> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS team_integration_schema_guard(version BIGINT PRIMARY KEY CHECK(version=1))",
        "CREATE TABLE IF NOT EXISTS team_mcp_servers(id TEXT PRIMARY KEY,team_id TEXT NOT NULL REFERENCES identity_organizations(id),name TEXT NOT NULL,revision BIGINT NOT NULL CHECK(revision>0),enabled BIGINT NOT NULL CHECK(enabled IN(0,1)),sealed_config TEXT NOT NULL,catalog TEXT,catalog_sha256 TEXT,sealed_session TEXT)",
        "CREATE TABLE IF NOT EXISTS team_github(team_id TEXT PRIMARY KEY REFERENCES identity_organizations(id),revision BIGINT NOT NULL CHECK(revision>0),hostname TEXT NOT NULL,enabled BIGINT NOT NULL CHECK(enabled IN(0,1)),key_configured BIGINT NOT NULL CHECK(key_configured IN(0,1)),sealed_config TEXT NOT NULL)",
    ] {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM team_integration_schema_guard")
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
    match versions.as_slice() {
        [] => {
            sqlx::query("INSERT INTO team_integration_schema_guard(version) VALUES(1)")
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        }
        [1] => {}
        _ => return Err(Error::Conflict),
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpConfiguration {
    endpoint: String,
    bearer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth: Option<crate::mcp_oauth::McpOAuthBinding>,
}
impl Drop for McpConfiguration {
    fn drop(&mut self) {
        if let Some(secret) = &mut self.bearer {
            secret.zeroize();
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubConfiguration {
    hostname: String,
    token: String,
}
impl Drop for GithubConfiguration {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

fn configuration_scope(team: Uuid, id: &str, revision: i64, purpose: &str) -> CredentialScope {
    CredentialScope {
        project_id: format!("team:{team}"),
        server_id: id.into(),
        reference: purpose.into(),
        revision,
        endpoint: format!("team-configuration://{purpose}"),
    }
}
fn seal_configuration<T: Serialize>(
    keys: &dyn KeyStore,
    key_id: KeyId,
    scope: &CredentialScope,
    value: &T,
) -> Result<String> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
    if bytes.len() > 3000 {
        return Err(invalid());
    }
    let encoded = Zeroizing::new(STANDARD.encode(bytes.as_slice()));
    json_string(
        &seal_bearer(keys, key_id, scope, &encoded).map_err(|_| Error::NativeKeyUnavailable)?,
    )
}
fn open_configuration<T: for<'a> Deserialize<'a>>(
    keys: &dyn KeyStore,
    key_id: KeyId,
    scope: &CredentialScope,
    sealed: &str,
) -> Result<T> {
    let sealed: SealedCredential = parse(sealed)?;
    if sealed.key_id() != key_id {
        return Err(Error::NativeKeyUnavailable);
    }
    let encoded = open_bearer(keys, scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    serde_json::from_slice(&bytes).map_err(|_| Error::NativeKeyUnavailable)
}
fn server_id(row: &AnyRow) -> Result<Uuid> {
    Uuid::parse_str(&row.try_get::<String, _>("id").map_err(storage_error)?).map_err(storage_error)
}
fn mcp_configuration(
    keys: &dyn KeyStore,
    key_id: KeyId,
    team: Uuid,
    row: &AnyRow,
) -> Result<McpConfiguration> {
    open_configuration(
        keys,
        key_id,
        &configuration_scope(
            team,
            &server_id(row)?.to_string(),
            row.try_get("revision").map_err(storage_error)?,
            "team-mcp/v1",
        ),
        &row.try_get::<String, _>("sealed_config")
            .map_err(storage_error)?,
    )
}
fn mcp_metadata(
    keys: &dyn KeyStore,
    key_id: KeyId,
    team: Uuid,
    row: &AnyRow,
) -> Result<TeamMcpServer> {
    let config = mcp_configuration(keys, key_id, team, row)?;
    let catalog: Option<String> = row.try_get("catalog").map_err(storage_error)?;
    let count = match catalog {
        Some(value) => parse::<Vec<Value>>(&value)?.len(),
        None => 0,
    };
    Ok(TeamMcpServer {
        id: server_id(row)?,
        team_id: team,
        name: row.try_get("name").map_err(storage_error)?,
        endpoint: config.endpoint.clone(),
        revision: row.try_get("revision").map_err(storage_error)?,
        enabled: row.try_get::<i64, _>("enabled").map_err(storage_error)? == 1,
        credential_configured: config.bearer.is_some() || config.oauth.is_some(),
        catalog_sha256: row.try_get("catalog_sha256").map_err(storage_error)?,
        tool_count: count,
    })
}
fn checkpoint_scope(
    team: Uuid,
    server: Uuid,
    revision: i64,
    catalog_sha256: &str,
) -> CheckpointScope {
    CheckpointScope {
        project_id: format!("team:{team}"),
        run_id: "team-discovery".into(),
        server_id: server.to_string(),
        connection_record_id: server.to_string(),
        server_revision: revision,
        checkpoint_revision: revision,
        configuration_sha256: hash(format!("{team}:{server}:{revision}").as_bytes()),
        catalog_sha256: catalog_sha256.into(),
    }
}
fn cipher(keys: &dyn KeyStore, key_id: KeyId) -> Result<SessionCipher> {
    SessionCipher::new(
        &key_id
            .cipher_id()
            .map_err(|_| Error::NativeKeyUnavailable)?,
        keys.load(&key_id)
            .map_err(|_| Error::NativeKeyUnavailable)?,
    )
    .map_err(|_| Error::NativeKeyUnavailable)
}

pub(super) async fn rewrap_server_secrets(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let rows = sqlx::query("SELECT * FROM team_mcp_servers ORDER BY id LIMIT 100001")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    let mut count = 0u64;
    for row in rows {
        let team = Uuid::parse_str(&row.try_get::<String, _>("team_id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let server = server_id(&row)?;
        let revision: i64 = row.try_get("revision").map_err(storage_error)?;
        let original: String = row.try_get("sealed_config").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance {
            return Err(Error::NativeKeyUnavailable);
        }
        let config_scope = configuration_scope(team, &server.to_string(), revision, "team-mcp/v1");
        let plain =
            open_bearer(keys, &config_scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let rewritten = seal_bearer(keys, target, &config_scope, &plain)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let original_session: Option<String> =
            row.try_get("sealed_session").map_err(storage_error)?;
        let session = original_session
            .as_ref()
            .map(|original| -> Result<String> {
                let sealed: SealedSession = parse(original)?;
                let source = KeyId::from_cipher_id(sealed.key_id())
                    .map_err(|_| Error::NativeKeyUnavailable)?;
                if source.instance != target.instance {
                    return Err(Error::NativeKeyUnavailable);
                }
                let catalog_sha: String = row
                    .try_get::<Option<String>, _>("catalog_sha256")
                    .map_err(storage_error)?
                    .ok_or(Error::Conflict)?;
                json_string(
                    &cipher(keys, source)?
                        .rewrap(
                            &sealed,
                            &checkpoint_scope(team, server, revision, &catalog_sha),
                            &cipher(keys, target)?,
                        )
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                )
            })
            .transpose()?;
        if sqlx::query("UPDATE team_mcp_servers SET sealed_config=$1,sealed_session=$2 WHERE id=$3 AND revision=$4 AND sealed_config=$5 AND COALESCE(sealed_session,'')=$6").bind(json_string(&rewritten)?).bind(session).bind(server.to_string()).bind(revision).bind(original).bind(original_session.unwrap_or_default()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        count += 1;
    }
    let rows = sqlx::query(
        "SELECT team_id,revision,sealed_config FROM team_github ORDER BY team_id LIMIT 100001",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    for row in rows {
        let team = Uuid::parse_str(&row.try_get::<String, _>("team_id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let revision: i64 = row.try_get("revision").map_err(storage_error)?;
        let original: String = row.try_get("sealed_config").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance {
            return Err(Error::NativeKeyUnavailable);
        }
        let scope = configuration_scope(team, "team-github", revision, "team-github/v1");
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let rewritten =
            seal_bearer(keys, target, &scope, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        if sqlx::query("UPDATE team_github SET sealed_config=$1 WHERE team_id=$2 AND revision=$3 AND sealed_config=$4").bind(json_string(&rewritten)?).bind(team.to_string()).bind(revision).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        count += 1;
    }
    Ok(count)
}
pub(super) fn mcp_alias(server: Uuid, name: &str) -> String {
    format!("mcp_{}_{}", server.simple(), &hash(name.as_bytes())[..16])
}
pub(super) fn is_mcp_alias(name: &str) -> bool {
    name.len() == 53
        && name.starts_with("mcp_")
        && name.as_bytes()[36] == b'_'
        && name.as_bytes()[4..36]
            .iter()
            .chain(&name.as_bytes()[37..])
            .all(|b| b.is_ascii_hexdigit())
}
fn catalog(row: &AnyRow) -> Result<Catalog> {
    let definitions: Vec<Value> = parse(
        &row.try_get::<Option<String>, _>("catalog")
            .map_err(storage_error)?
            .ok_or(Error::Conflict)?,
    )?;
    let result = Catalog::from_definitions(definitions).map_err(|_| Error::Conflict)?;
    if row
        .try_get::<Option<String>, _>("catalog_sha256")
        .map_err(storage_error)?
        .as_deref()
        != Some(result.sha256())
    {
        return Err(Error::Conflict);
    }
    Ok(result)
}
fn public_tools(server: Uuid, catalog: &Catalog) -> Vec<TeamMcpTool> {
    catalog
        .tools()
        .map(|(name, tool)| TeamMcpTool {
            name: mcp_alias(server, name),
            remote_name: name.into(),
            description: tool
                .definition()
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            input_schema: tool.definition()["inputSchema"].clone(),
        })
        .collect()
}
fn redact_response(value: &mut Value, secret: Option<&str>) {
    let Some(secret) = secret.filter(|s| !s.is_empty()) else {
        return;
    };
    match value {
        Value::String(s) => {
            if s.contains(secret) {
                *s = s.replace(secret, "[redacted]");
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_response(value, Some(secret));
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_response(value, Some(secret));
            }
        }
        _ => {}
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct McpRunBinding {
    revision: i64,
    catalog_sha256: String,
    tools: BTreeSet<String>,
}

impl McpRunBinding {
    pub(super) fn execution_review(
        &self,
        server_id: Uuid,
    ) -> cowork_workflow_contracts::TeamExecutionMcpBinding {
        cowork_workflow_contracts::TeamExecutionMcpBinding {
            server_id,
            revision: self.revision,
            catalog_sha256: self.catalog_sha256.clone(),
            tools: self.tools.iter().cloned().collect(),
        }
    }
}

impl TeamService {
    /// Exact origins are supplied by the server operator, not a team or model.
    pub fn with_mcp_origins(mut self, origins: Vec<String>) -> Result<Self> {
        if origins.len() > 64 {
            return Err(invalid());
        }
        for value in origins {
            let endpoint = Endpoint::new(&value, None).map_err(|_| invalid())?;
            let url = reqwest::Url::parse(endpoint.url()).map_err(|_| invalid())?;
            if url.path() != "/" {
                return Err(invalid());
            }
            self.allowed_mcp_origins
                .insert(url.origin().ascii_serialization());
        }
        Ok(self)
    }
    fn validate_mcp_endpoint(&self, value: &str) -> Result<String> {
        if value.len() > 1024 {
            return Err(invalid());
        }
        let endpoint = Endpoint::new(value, None).map_err(|_| invalid())?;
        let url = reqwest::Url::parse(endpoint.url()).map_err(|_| invalid())?;
        if !self
            .allowed_mcp_origins
            .contains(&url.origin().ascii_serialization())
        {
            return Err(Error::Forbidden);
        }
        Ok(url.to_string())
    }
    pub async fn mcp_servers(&self, actor: &Principal, team: Uuid) -> Result<Vec<TeamMcpServer>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.store
            .identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        let rows = sqlx::query("SELECT * FROM team_mcp_servers WHERE team_id=$1 ORDER BY name,id")
            .bind(team.to_string())
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = rows
            .iter()
            .map(|row| mcp_metadata(self.keys.as_ref(), self.key_id, team, row))
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn configure_mcp_server(
        &self,
        actor: &Principal,
        team: Uuid,
        id: Option<Uuid>,
        input: ConfigureTeamMcpServer,
    ) -> Result<TeamMcpServer> {
        safe_text(&input.name, 200)?;
        if let (Some(route), Some(body)) = (id, input.id)
            && route != body
        {
            return Err(invalid());
        }
        let server = id.or(input.id).unwrap_or_else(Uuid::new_v4);
        if server.is_nil() {
            return Err(invalid());
        }
        let endpoint = if input.enabled {
            self.validate_mcp_endpoint(&input.endpoint)?
        } else {
            if input.endpoint.len() > 1024 {
                return Err(invalid());
            }
            Endpoint::new(&input.endpoint, None)
                .map_err(|_| invalid())?
                .url()
                .to_string()
        };
        let incoming = input.bearer.map(Zeroizing::new);
        if incoming.as_ref().is_some_and(|s| s.len() > 1800) {
            return Err(invalid());
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.manager(&mut tx, actor, team, false).await?;
        let old = sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1")
            .bind(server.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        if old.as_ref().is_some_and(|row| {
            row.try_get::<String, _>("team_id").ok().as_deref() != Some(team.to_string().as_str())
        }) {
            return Err(Error::Forbidden);
        }
        let revision = old
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0);
        if revision == 0 {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM team_mcp_servers WHERE team_id=$1")
                    .bind(team.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            if count >= 64 {
                return Err(invalid());
            }
        }
        let previous = old
            .as_ref()
            .map(|r| mcp_configuration(self.keys.as_ref(), self.key_id, team, r))
            .transpose()?;
        let bearer = match incoming.as_deref() {
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s.to_string()),
            None => previous.as_ref().and_then(|c| c.bearer.clone()),
        };
        Endpoint::new(&endpoint, bearer.as_deref()).map_err(|_| invalid())?;
        let config = McpConfiguration {
            endpoint,
            bearer,
            oauth: None,
        };
        if revision == 1
            && input.expected_revision == 0
            && let (Some(row), Some(previous)) = (&old, &previous)
            && previous.endpoint == config.endpoint
            && previous.bearer == config.bearer
            && row.try_get::<String, _>("name").map_err(storage_error)? == input.name.trim()
            && row.try_get::<i64, _>("enabled").map_err(storage_error)? == i64::from(input.enabled)
        {
            let result = mcp_metadata(self.keys.as_ref(), self.key_id, team, row)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        if revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let next = revision.checked_add(1).ok_or(Error::Conflict)?;
        let sealed = seal_configuration(
            self.keys.as_ref(),
            self.key_id,
            &configuration_scope(team, &server.to_string(), next, "team-mcp/v1"),
            &config,
        )?;
        let affected=sqlx::query("INSERT INTO team_mcp_servers(id,team_id,name,revision,enabled,sealed_config,catalog,catalog_sha256,sealed_session) VALUES($1,$2,$3,$4,$5,$6,NULL,NULL,NULL) ON CONFLICT(id) DO UPDATE SET name=excluded.name,revision=excluded.revision,enabled=excluded.enabled,sealed_config=excluded.sealed_config,catalog=NULL,catalog_sha256=NULL,sealed_session=NULL WHERE team_mcp_servers.team_id=excluded.team_id").bind(server.to_string()).bind(team.to_string()).bind(input.name.trim()).bind(next).bind(i64::from(input.enabled)).bind(sealed).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if affected != 1 {
            return Err(Error::Conflict);
        }
        let row = sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2")
            .bind(server.to_string())
            .bind(team.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = mcp_metadata(self.keys.as_ref(), self.key_id, team, &row)?;
        self.manager(&mut tx, actor, team, false).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn discover_mcp_server(
        &self,
        actor: &Principal,
        team: Uuid,
        server: Uuid,
    ) -> Result<TeamMcpServer> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.manager(&mut tx, actor, team, false).await?;
        let row =
            sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND enabled=1")
                .bind(server.to_string())
                .bind(team.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let revision: i64 = row.try_get("revision").map_err(storage_error)?;
        let config = mcp_configuration(self.keys.as_ref(), self.key_id, team, &row)?;
        self.validate_mcp_endpoint(&config.endpoint)?;
        tx.commit().await.map_err(storage_error)?;
        let oauth_bearer = if let Some(oauth) = &config.oauth {
            Some(
                self.store
                    .mcp_oauth_bearer_for(
                        &crate::mcp_oauth::McpOAuthOwner::Team {
                            actor,
                            team,
                            key: self.key_id,
                        },
                        &oauth.id,
                        self.keys.as_ref(),
                    )
                    .await?,
            )
        } else {
            None
        };
        let mut session = Session::connect_interactive(
            Endpoint::new(
                &config.endpoint,
                oauth_bearer
                    .as_deref()
                    .map(|v| v.as_str())
                    .or(config.bearer.as_deref()),
            )
            .map_err(|_| invalid())?,
            Limits::default(),
        )
        .await
        .map_err(|_| Error::Provider("mcp_discovery_failed".into()))?;
        let discovered = session
            .discover()
            .await
            .map_err(|_| Error::Provider("mcp_discovery_failed".into()))?;
        let definitions: Vec<Value> = discovered
            .tools()
            .map(|(_, tool)| tool.definition().clone())
            .collect();
        let sha = discovered.sha256().to_string();
        let next = revision.checked_add(1).ok_or(Error::Conflict)?;
        let checkpoint = cipher(self.keys.as_ref(), self.key_id)?
            .seal(&session, &checkpoint_scope(team, server, next, &sha))
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let sealed = seal_configuration(
            self.keys.as_ref(),
            self.key_id,
            &configuration_scope(team, &server.to_string(), next, "team-mcp/v1"),
            &config,
        )?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.manager(&mut tx, actor, team, false).await?;
        let changed=sqlx::query("UPDATE team_mcp_servers SET revision=$1,sealed_config=$2,catalog=$3,catalog_sha256=$4,sealed_session=$5 WHERE id=$6 AND team_id=$7 AND revision=$8 AND enabled=1").bind(next).bind(sealed).bind(json_string(&definitions)?).bind(&sha).bind(json_string(&checkpoint)?).bind(server.to_string()).bind(team.to_string()).bind(revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let row = sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2")
            .bind(server.to_string())
            .bind(team.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = mcp_metadata(self.keys.as_ref(), self.key_id, team, &row)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn mcp_tools(
        &self,
        actor: &Principal,
        team: Uuid,
        server: Uuid,
    ) -> Result<Vec<TeamMcpTool>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.store
            .identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        let row =
            sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND enabled=1")
                .bind(server.to_string())
                .bind(team.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let result = if row
            .try_get::<Option<String>, _>("catalog")
            .map_err(storage_error)?
            .is_some()
        {
            public_tools(server, &catalog(&row)?)
        } else {
            Vec::new()
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn github(
        &self,
        actor: &Principal,
        team: Uuid,
    ) -> Result<Option<TeamGithubConfiguration>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        // The nonsecret hostname/capability metadata is needed by members when
        // requesting repository actions. Updating credentials remains manager-only.
        self.store
            .identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        let row = sqlx::query("SELECT * FROM team_github WHERE team_id=$1")
            .bind(team.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = row.as_ref().map(|r| github_metadata(team, r)).transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn configure_github(
        &self,
        actor: &Principal,
        team: Uuid,
        input: ConfigureTeamGithub,
    ) -> Result<TeamGithubConfiguration> {
        let hostname = github_hostname(&input.hostname)?;
        let incoming = Zeroizing::new(input.token);
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.manager(&mut tx, actor, team, false).await?;
        let old = sqlx::query("SELECT * FROM team_github WHERE team_id=$1")
            .bind(team.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let revision = old
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0);
        if revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let token = if incoming.is_empty() {
            match &old {
                Some(r) => open_configuration::<GithubConfiguration>(
                    self.keys.as_ref(),
                    self.key_id,
                    &configuration_scope(team, "team-github", revision, "team-github/v1"),
                    &r.try_get::<String, _>("sealed_config")
                        .map_err(storage_error)?,
                )?
                .token
                .clone(),
                None => String::new(),
            }
        } else {
            incoming.to_string()
        };
        if input.enabled && token.is_empty() || token.len() > 2048 {
            return Err(invalid());
        }
        if !token.is_empty() {
            cowork_secrets::validate_bearer(&token).map_err(|_| invalid())?;
        }
        let config = GithubConfiguration {
            hostname: hostname.clone(),
            token,
        };
        let next = revision.checked_add(1).ok_or(Error::Conflict)?;
        let sealed = seal_configuration(
            self.keys.as_ref(),
            self.key_id,
            &configuration_scope(team, "team-github", next, "team-github/v1"),
            &config,
        )?;
        sqlx::query("INSERT INTO team_github(team_id,revision,hostname,enabled,key_configured,sealed_config) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(team_id) DO UPDATE SET revision=excluded.revision,hostname=excluded.hostname,enabled=excluded.enabled,key_configured=excluded.key_configured,sealed_config=excluded.sealed_config").bind(team.to_string()).bind(next).bind(&hostname).bind(i64::from(input.enabled)).bind(i64::from(!config.token.is_empty())).bind(sealed).execute(&mut *tx).await.map_err(storage_error)?;
        let result = TeamGithubConfiguration {
            team_id: team,
            revision: next,
            hostname,
            enabled: input.enabled,
            key_configured: !config.token.is_empty(),
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
fn github_hostname(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(&format!("https://{value}/")).map_err(|_| invalid())?;
    if value.is_empty()
        || value.len() > 253
        || value.contains(['/', ':', '@'])
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        || url.host_str() != Some(value)
    {
        return Err(invalid());
    }
    Ok(value.into())
}
fn github_metadata(team: Uuid, row: &AnyRow) -> Result<TeamGithubConfiguration> {
    Ok(TeamGithubConfiguration {
        team_id: team,
        revision: row.try_get("revision").map_err(storage_error)?,
        hostname: row.try_get("hostname").map_err(storage_error)?,
        enabled: row.try_get::<i64, _>("enabled").map_err(storage_error)? == 1,
        key_configured: row
            .try_get::<i64, _>("key_configured")
            .map_err(storage_error)?
            == 1,
    })
}
pub(super) async fn available_mcp_tools(
    tx: &mut Transaction<'_, Any>,
    team: Uuid,
) -> Result<Vec<String>> {
    let rows=sqlx::query("SELECT * FROM team_mcp_servers WHERE team_id=$1 AND enabled=1 AND catalog IS NOT NULL AND sealed_session IS NOT NULL").bind(team.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    let mut names = Vec::new();
    for row in rows {
        names.extend(
            public_tools(server_id(&row)?, &catalog(&row)?)
                .into_iter()
                .map(|t| t.name),
        );
    }
    names.sort();
    Ok(names)
}
pub(super) async fn freeze_tools(
    tx: &mut Transaction<'_, Any>,
    team: Uuid,
    names: &[String],
) -> Result<(BTreeMap<Uuid, McpRunBinding>, Option<i64>)> {
    let mut bindings = BTreeMap::new();
    for name in names.iter().filter(|s| is_mcp_alias(s)) {
        let server = Uuid::parse_str(&name[4..36]).map_err(|_| invalid())?;
        let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND enabled=1 AND catalog IS NOT NULL AND sealed_session IS NOT NULL").bind(server.to_string()).bind(team.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let c = catalog(&row)?;
        if !c.tools().any(|(tool, _)| mcp_alias(server, tool) == *name) {
            return Err(Error::Forbidden);
        }
        let binding = bindings.entry(server).or_insert(McpRunBinding {
            revision: row.try_get("revision").map_err(storage_error)?,
            catalog_sha256: c.sha256().into(),
            tools: BTreeSet::new(),
        });
        binding.tools.insert(name.clone());
    }
    let github = if names.iter().any(|s| s == "github") {
        Some(sqlx::query_scalar("SELECT revision FROM team_github WHERE team_id=$1 AND enabled=1 AND key_configured=1")
            .bind(team.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?.ok_or(Error::Validation("Configure the team's GitHub credential first".into()))?)
    } else {
        None
    };
    Ok((bindings, github))
}
pub(super) async fn validate_integrations(
    tx: &mut Transaction<'_, Any>,
    binding: &ExecutionBinding,
) -> Result<()> {
    for (server, expected) in &binding.mcp_servers {
        let row=sqlx::query("SELECT revision,catalog_sha256,enabled FROM team_mcp_servers WHERE id=$1 AND team_id=$2").bind(server.to_string()).bind(binding.team_id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if row.try_get::<i64, _>("revision").map_err(storage_error)? != expected.revision
            || row.try_get::<i64, _>("enabled").map_err(storage_error)? != 1
            || row
                .try_get::<Option<String>, _>("catalog_sha256")
                .map_err(storage_error)?
                .as_deref()
                != Some(&expected.catalog_sha256)
        {
            return Err(Error::Forbidden);
        }
    }
    if let Some(expected) = binding.github_revision {
        let actual: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM team_github WHERE team_id=$1 AND enabled=1")
                .bind(binding.team_id.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?;
        if actual != Some(expected) {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

pub struct TeamGithubCredential {
    pub revision: i64,
    pub hostname: String,
    pub token: Zeroizing<String>,
}
impl TeamExecutionAuthority {
    pub(crate) async fn mcp_oauth_execution_fence(
        &self,
        tx: &mut Transaction<'_, Any>,
        project: &str,
        server: &str,
        revision: Option<i64>,
        resource: Option<&str>,
        keys: &dyn KeyStore,
    ) -> Result<String> {
        validate_binding(&self.store, tx, &self.binding, &self.lease_owner).await?;
        if project != format!("team:{}", self.team_id()) {
            return Err(Error::Forbidden);
        }
        let server_id = Uuid::parse_str(server).map_err(|_| invalid())?;
        let binding = self
            .binding
            .mcp_servers
            .get(&server_id)
            .ok_or(Error::Forbidden)?;
        if revision.is_some_and(|v| v != binding.revision) {
            return Err(Error::Forbidden);
        }
        let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND revision=$3 AND enabled=1").bind(server).bind(self.team_id().to_string()).bind(binding.revision).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let config = mcp_configuration(keys, self.key_id, self.team_id(), &row)?;
        let url = reqwest::Url::parse(&config.endpoint).map_err(|_| invalid())?;
        if !self
            .allowed_mcp_origins
            .contains(&url.origin().ascii_serialization())
            || resource.is_some_and(|v| v != config.endpoint)
        {
            return Err(Error::Forbidden);
        }
        Ok(canonical_hash(&json!([
            self.binding,
            self.lease_owner,
            server,
            binding.revision,
            config.endpoint,
            config.oauth
        ])))
    }
    pub async fn github_metadata(&self) -> Result<Option<TeamGithubConfiguration>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        let row =
            sqlx::query("SELECT * FROM team_github WHERE team_id=$1 AND enabled=1 AND revision=$2")
                .bind(self.team_id().to_string())
                .bind(self.binding.github_revision.unwrap_or(0))
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        let result = row
            .as_ref()
            .map(|r| github_metadata(self.team_id(), r))
            .transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn github_credential(&self, approval: Uuid) -> Result<Option<TeamGithubCredential>> {
        self.revalidate_tool_approval(approval).await?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        let result = match self.binding.github_revision {
            None => None,
            Some(revision) => {
                let row = sqlx::query(
                    "SELECT * FROM team_github WHERE team_id=$1 AND enabled=1 AND revision=$2",
                )
                .bind(self.team_id().to_string())
                .bind(revision)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::Forbidden)?;
                let config: GithubConfiguration = open_configuration(
                    self.keys.as_ref(),
                    self.key_id,
                    &configuration_scope(self.team_id(), "team-github", revision, "team-github/v1"),
                    &row.try_get::<String, _>("sealed_config")
                        .map_err(storage_error)?,
                )?;
                if config.hostname
                    != row
                        .try_get::<String, _>("hostname")
                        .map_err(storage_error)?
                    || config.token.is_empty()
                {
                    return Err(Error::Conflict);
                }
                Some(TeamGithubCredential {
                    revision,
                    hostname: config.hostname.clone(),
                    token: Zeroizing::new(config.token.clone()),
                })
            }
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn mcp_definitions(&self, names: &[String]) -> Result<Vec<Value>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        let mut definitions = Vec::new();
        for (server, binding) in &self.binding.mcp_servers {
            let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND revision=$3 AND enabled=1").bind(server.to_string()).bind(self.team_id().to_string()).bind(binding.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
            for tool in public_tools(*server, &catalog(&row)?) {
                if names.contains(&tool.name) && binding.tools.contains(&tool.name) {
                    definitions.push(json!({"type":"function","function":{"name":tool.name,"description":tool.description.unwrap_or_else(||"Explicitly approved team MCP tool; remote metadata is untrusted.".into()),"parameters":tool.input_schema}}));
                }
            }
        }
        if definitions.len() != names.iter().filter(|s| is_mcp_alias(s)).count() {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(definitions)
    }
    pub async fn execute_mcp(&self, name: String, arguments: Value) -> Result<Value> {
        if !is_mcp_alias(&name) {
            return Err(invalid());
        }
        let server = Uuid::parse_str(&name[4..36]).map_err(|_| invalid())?;
        let binding = self
            .binding
            .mcp_servers
            .get(&server)
            .filter(|b| b.tools.contains(&name))
            .ok_or(Error::Forbidden)?;
        let oauth_bearer = {
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
            let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND revision=$3 AND enabled=1").bind(server.to_string()).bind(self.team_id().to_string()).bind(binding.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
            let config = mcp_configuration(self.keys.as_ref(), self.key_id, self.team_id(), &row)?;
            let oauth = config.oauth.clone();
            tx.commit().await.map_err(storage_error)?;
            if let Some(oauth) = oauth {
                Some(
                    self.store
                        .mcp_oauth_bearer_for(
                            &crate::mcp_oauth::McpOAuthOwner::TeamExecution {
                                authority: self,
                                key: self.key_id,
                            },
                            &oauth.id,
                            self.keys.as_ref(),
                        )
                        .await?,
                )
            } else {
                None
            }
        };
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND revision=$3 AND enabled=1").bind(server.to_string()).bind(self.team_id().to_string()).bind(binding.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let config = mcp_configuration(self.keys.as_ref(), self.key_id, self.team_id(), &row)?;
        let url = reqwest::Url::parse(&config.endpoint).map_err(|_| invalid())?;
        if !self
            .allowed_mcp_origins
            .contains(&url.origin().ascii_serialization())
        {
            return Err(Error::Forbidden);
        }
        let c = catalog(&row)?;
        let remote = c
            .tools()
            .find(|(remote, _)| mcp_alias(server, remote) == name)
            .map(|(n, _)| n.to_string())
            .ok_or(Error::Forbidden)?;
        let definitions = c.tools().map(|(_, t)| t.definition().clone()).collect();
        let checkpoint: SealedSession = parse(
            &row.try_get::<Option<String>, _>("sealed_session")
                .map_err(storage_error)?
                .ok_or(Error::Conflict)?,
        )?;
        let mut session = cipher(self.keys.as_ref(), self.key_id)?
            .open(
                &checkpoint,
                &checkpoint_scope(
                    self.team_id(),
                    server,
                    binding.revision,
                    &binding.catalog_sha256,
                ),
                Endpoint::new(
                    &config.endpoint,
                    oauth_bearer
                        .as_deref()
                        .map(|v| v.as_str())
                        .or(config.bearer.as_deref()),
                )
                .map_err(|_| invalid())?,
                Limits::default(),
                definitions,
            )
            .map_err(|_| Error::Conflict)?;
        let prepared = session
            .prepare(&binding.catalog_sha256, &remote, arguments)
            .map_err(|_| invalid())?;
        if prepared.task_ttl_ms().is_some() && self.effect.is_none() {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        let mut action = json!({"mode":"team_mcp","team_id":self.team_id(),"project_id":self.project_id(),"order_id":self.order_id(),"run_id":self.run_id(),"server_id":server,"server_revision":binding.revision,"endpoint":prepared.endpoint(),"catalog_sha256":binding.catalog_sha256,"connection_id":prepared.connection_id(),"rpc_request_id":prepared.rpc_request_id(),"transport_request_sha256":prepared.request_sha256(),"tool":remote,"arguments":prepared.arguments()});
        if let Some(ttl) = prepared.task_ttl_ms() {
            action["task"] = json!({"ttl":ttl});
            action["epoch"] = json!(prepared.epoch());
            action["protocol"] = json!(prepared.protocol());
        }
        let approval_sha = hash(json_string(&action)?.as_bytes());
        let approval = self
            .request_tool_approval(&name, action.clone(), approval_sha.clone())
            .await?;
        let started = tokio::time::Instant::now();
        loop {
            self.revalidate().await?;
            match self.tool_approval(approval.id).await?.state.as_str() {
                "approved" => break,
                "pending" => {}
                "rejected" | "expired" => return Err(Error::Forbidden),
                _ => return Err(Error::UncertainEffect),
            }
            if started.elapsed() > Duration::from_secs(180) {
                let _ = self
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
        self.claim_tool_approval(approval.id, &approval_sha).await?;
        self.revalidate_tool_approval(approval.id).await?;
        // This one-shot call cannot reconnect/retry. A dropped or failed call
        // retains its executing approval and the run becomes unknown on recovery.
        let result = session.call(prepared).await;
        let mut result = match result {
            Ok(received) if received.task.is_some() => self
                .await_team_mcp_task(
                    server,
                    approval.id,
                    action,
                    approval_sha,
                    &mut session,
                    received,
                )
                .await
                .map_err(|_| ()),
            Ok(received) => Ok(received),
            Err(_) => Err(()),
        };
        if let Ok(received) = &mut result {
            redact_response(
                &mut received.response,
                oauth_bearer
                    .as_deref()
                    .map(|v| v.as_str())
                    .or(config.bearer.as_deref()),
            );
            self.record_tool_result(&received.response).await?;
        }
        self.revalidate_tool_approval(approval.id).await?;
        match result {
            Ok(mut result) => {
                if result.catalog_changed {
                    let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
                    validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner)
                        .await?;
                    sqlx::query("UPDATE team_mcp_servers SET catalog=NULL,catalog_sha256=NULL,sealed_session=NULL WHERE id=$1 AND team_id=$2 AND revision=$3").bind(server.to_string()).bind(self.team_id().to_string()).bind(binding.revision).execute(&mut *tx).await.map_err(storage_error)?;
                    tx.commit().await.map_err(storage_error)?;
                    return Err(Error::UncertainEffect);
                }
                self.finish_tool_approval(
                    approval.id,
                    Some(result.response_sha256),
                    "completed",
                    None,
                )
                .await?;
                redact_response(
                    &mut result.response,
                    oauth_bearer
                        .as_deref()
                        .map(|v| v.as_str())
                        .or(config.bearer.as_deref()),
                );
                Ok(result.response)
            }
            Err(_) => {
                let _ = self
                    .finish_tool_approval(
                        approval.id,
                        None,
                        "unknown",
                        Some("mcp_call_unconfirmed".into()),
                    )
                    .await;
                Err(Error::UncertainEffect)
            }
        }
    }
}
