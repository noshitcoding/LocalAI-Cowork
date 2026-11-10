use super::*;
use cowork_mcp::{Endpoint, Limits, Session};

impl Runtime {
    /// Only NEW owner metadata handshakes may be retried. This method never calls
    /// tools/call, restores a session, changes its epoch or clears an attempt.
    pub async fn connect_owner_mcp_catalog(
        &self,
        server: &str,
        url: &str,
        bearer: Option<&str>,
        limits: Limits,
        expected_catalog: Option<&str>,
        interactive: bool,
    ) -> Result<Session> {
        let preferences = self
            .store
            .library_preferences(cowork_storage::library_config::LibraryOwner::Local)
            .await?
            .value;
        self.connect_owner_mcp_catalog_with_preferences(
            server,
            url,
            bearer,
            limits,
            expected_catalog,
            interactive,
            &preferences,
        )
        .await
    }
    // Callers holding an MCP connection permit must not acquire another pooled
    // connection: SQLite deliberately has one. Load these options before admission.
    #[expect(
        clippy::too_many_arguments,
        reason = "connect_owner_mcp_catalog_with_preferences: Keep selected provider, sealed credential scope and MCP session/lease authority explicit through lifecycle preparation"
    )]
    pub(crate) async fn connect_owner_mcp_catalog_with_preferences(
        &self,
        server: &str,
        url: &str,
        bearer: Option<&str>,
        limits: Limits,
        expected_catalog: Option<&str>,
        interactive: bool,
        preferences: &ApplicationPreferences,
    ) -> Result<Session> {
        let diagnostic_server = uuid::Uuid::parse_str(server)
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "non_uuid_server".into());
        let attempts = if preferences.mcp_auto_reconnect { 3 } else { 1 };
        for attempt in 1..=attempts {
            if preferences.mcp_verbose_logging {
                tracing::info!(target:"cowork::mcp_lifecycle",server_id=diagnostic_server.as_str(),attempt,phase="new_handshake","MCP lifecycle");
            }
            let result = async {
                let endpoint = Endpoint::new(url, bearer)?;
                let mut session = if interactive {
                    Session::connect_interactive(endpoint, limits).await?
                } else {
                    Session::connect(endpoint, limits).await?
                };
                session.discover().await?;
                if expected_catalog.is_some_and(|expected| {
                    session.catalog().map(cowork_mcp::Catalog::sha256) != Some(expected)
                }) {
                    return Err(cowork_mcp::Error::CatalogChanged);
                }
                Ok::<_, cowork_mcp::Error>(session)
            }
            .await;
            match result {
                Ok(session) => {
                    if preferences.mcp_verbose_logging {
                        tracing::info!(target:"cowork::mcp_lifecycle",server_id=diagnostic_server.as_str(),attempt,phase="catalog_ready","MCP lifecycle");
                    }
                    return Ok(session);
                }
                Err(code) => {
                    if preferences.mcp_verbose_logging {
                        tracing::info!(target:"cowork::mcp_lifecycle",server_id=diagnostic_server.as_str(),attempt,error_code=%code,phase="handshake_failed","MCP lifecycle");
                    }
                    if attempt == attempts
                        || !matches!(
                            code,
                            cowork_mcp::Error::Transport
                                | cowork_mcp::Error::Deadline
                                | cowork_mcp::Error::SessionUnavailable
                        )
                    {
                        return Err(Error::McpSessionUnavailable);
                    }
                    tokio::time::sleep(Duration::from_millis(150 * attempt)).await;
                }
            }
        }
        Err(Error::McpSessionUnavailable)
    }
}
