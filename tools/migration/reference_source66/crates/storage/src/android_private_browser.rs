//! Actor-private live browser ownership. Network permits are original, single-use observations.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use crate::android_private_server::{id, leased, row};
use crate::android_private_tools as tools;
use crate::android_private_workspace as workspace;
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction, any::AnyRow};

pub struct BrowserAdmission {
    pub session: Uuid,
    pub sequence: i64,
    pub invocation: String,
    pub request_sha256: String,
    pub action: Value,
    pub new_session: bool,
}
fn action(tool: &ToolCall) -> Result<Value> {
    if tool.name != "browser" {
        return Err(Error::Forbidden);
    }
    let fields = tool.arguments.as_object().ok_or_else(device_invalid)?;
    if fields.keys().any(|k| {
        !matches!(
            k.as_str(),
            "session_id"
                | "operation"
                | "url"
                | "selector"
                | "text"
                | "allowed_origins"
                | "allowed_methods"
        )
    }) {
        return Err(device_invalid());
    }
    let op = tool.arguments["operation"]
        .as_str()
        .filter(|s| {
            matches!(
                *s,
                "navigate" | "dom" | "screenshot" | "click" | "fill" | "close"
            )
        })
        .ok_or_else(device_invalid)?;
    if let Some(s) = fields.get("session_id") {
        Uuid::parse_str(s.as_str().ok_or_else(device_invalid)?).map_err(|_| device_invalid())?;
    }
    if op == "navigate" {
        url(tool.arguments["url"].as_str().ok_or_else(device_invalid)?)?;
    } else if fields.contains_key("url") {
        return Err(device_invalid());
    }
    if matches!(op, "click" | "fill")
        && tool.arguments["selector"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 4096 || s.contains('\0'))
    {
        return Err(device_invalid());
    }
    if op == "fill"
        && tool.arguments["text"]
            .as_str()
            .is_none_or(|s| s.len() > 32768 || s.contains('\0'))
    {
        return Err(device_invalid());
    }
    let origins = tool.arguments["allowed_origins"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 16)
        .ok_or_else(device_invalid)?;
    let mut names = std::collections::BTreeSet::new();
    for origin in origins {
        let s = origin.as_str().ok_or_else(device_invalid)?;
        let u = url(s)?;
        if u.origin().ascii_serialization() != s || !names.insert(s) {
            return Err(device_invalid());
        }
    }
    if op == "navigate"
        && !names.contains(
            url(tool.arguments["url"].as_str().ok_or_else(device_invalid)?)?
                .origin()
                .ascii_serialization()
                .as_str(),
        )
    {
        return Err(Error::Forbidden);
    }
    let methods = tool.arguments["allowed_methods"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 7)
        .ok_or_else(device_invalid)?;
    let mut unique = std::collections::BTreeSet::new();
    for method in methods {
        let m = method
            .as_str()
            .filter(|s| {
                matches!(
                    *s,
                    "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
                )
            })
            .ok_or_else(device_invalid)?;
        if !unique.insert(m) {
            return Err(device_invalid());
        }
    }
    Ok(tool.arguments.clone())
}
pub fn url(raw: &str) -> Result<reqwest::Url> {
    let u = reqwest::Url::parse(raw).map_err(|_| device_invalid())?;
    let host = u.host_str().ok_or_else(device_invalid)?;
    if raw.len() > 8192
        || !u.username().is_empty()
        || u.password().is_some()
        || !matches!(
            (u.scheme(), u.port_or_known_default()),
            ("https", Some(443)) | ("http", Some(80))
        )
        || !host.contains('.')
        || host.ends_with('.')
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.chars().any(char::is_control)
    {
        return Err(Error::Forbidden);
    }
    Ok(u)
}
async fn original(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    sequence: i64,
    keys: &dyn KeyStore,
) -> Result<Value> {
    let v:String=sqlx::query_scalar("SELECT intent FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND kind='process'").bind(id(r,"id")?.to_string()).bind(sequence).fetch_one(&mut **tx).await.map_err(storage_error)?;
    serde_json::from_slice(&workspace::open_value(
        keys,
        r,
        &format!("intent:{sequence}"),
        &parse(&v)?,
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
    let s=sqlx::query("SELECT * FROM android_private_browser_sessions WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='admitted' AND last_sequence=$4").bind(session.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(sequence).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if s.try_get::<String, _>("owner_id").map_err(storage_error)? != id(&r, "owner_id")?.to_string()
        || s.try_get::<String, _>("project_id")
            .map_err(storage_error)?
            != id(&r, "project_id")?.to_string()
    {
        return Err(Error::Forbidden);
    }
    let intent = original(tx, &r, sequence, keys).await?;
    let step=sqlx::query("SELECT checkpoint_sha,request_sha,state FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3").bind(run.to_string()).bind(sequence).bind(lease.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let tool = tools::pending(&cp)?;
    if intent["tool"] != json!(tool)
        || intent["action"]["session_id"] != session.to_string()
        || step
            .try_get::<String, _>("request_sha")
            .map_err(storage_error)?
            != canonical_hash(&intent)
        || intent["fingerprint"] != tools::tool_fingerprint(run, &cp, &tool)
        || step
            .try_get::<String, _>("checkpoint_sha")
            .map_err(storage_error)?
            != canonical_hash(&cp)
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
impl Store {
    /// Physical removal closes lost session ownership, without inventing completion or promoting files.
    pub async fn android_private_browser_recover(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        sqlx::query("UPDATE android_private_browser_sessions SET state='closed',revision=revision+1 WHERE state!='closed' AND container_nonce IN (SELECT c.id FROM android_private_containers c JOIN android_private_server_runs r ON r.id=c.run_id WHERE c.state='removed' AND c.termination_witness IS NOT NULL AND r.lease_until<=$1 AND r.state NOT IN('queued','running','waiting_for_approval','waiting_for_input'))").bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE android_private_browser_requests SET state='uncertain' WHERE state='admitted' AND session_id IN (SELECT id FROM android_private_browser_sessions WHERE state='closed')").execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_gate(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<crate::android_private_mcp_runtime::PrivateMcpGate> {
        use crate::android_private_mcp_runtime::PrivateMcpGate;
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(&mut tx, run, lease, now).await?;
        let mut cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(&mut tx, &r, &cp, now).await?;
        let tool = tools::pending(&cp)?;
        action(&tool)?;
        let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if original["spec"]["input"]["frozen_private_runner"]["browser"] != true
            || !cp["spec"]["allowed_tools"]
                .as_array()
                .is_some_and(|v| v.contains(&json!("browser")))
        {
            return Err(Error::Forbidden);
        }
        let req = tools::request(&mut tx, &r, &cp, &tool, "approval", now).await?;
        let gate = match req["state"].as_str() {
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
                    key_id,
                    now,
                )
                .await?,
            ),
            _ => return Err(Error::Forbidden),
        };
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(gate)
    }
    pub async fn android_private_browser_admit(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<BrowserAdmission> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = leased(&mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(&mut tx, &r, &cp, now).await?;
        let tool = tools::pending(&cp)?;
        let mut action = action(&tool)?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        if !spec.allowed_tools.iter().any(|s| s == "browser")
            || cp["model_steps"].as_u64().ok_or_else(device_invalid)?
                + cp["tool_steps"].as_u64().ok_or_else(device_invalid)?
                >= u64::from(spec.budgets.max_steps)
        {
            return Err(Error::BudgetExceeded);
        }
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
        let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let runner = &original["spec"]["input"]["frozen_private_runner"];
        if runner["browser"] != true {
            return Err(Error::Forbidden);
        }
        let new_session = action.get("session_id").is_none();
        let session = if new_session {
            if action["operation"] != "navigate" {
                return Err(device_invalid());
            }
            Uuid::new_v4()
        } else {
            Uuid::parse_str(action["session_id"].as_str().ok_or_else(device_invalid)?)
                .map_err(|_| device_invalid())?
        };
        action["session_id"] = json!(session);
        let sequence = tools::next_sequence(&mut tx, run).await?;
        if new_session {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_browser_sessions WHERE run_id=$1 AND state!='closed'").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count >= 4 {
                return Err(Error::BudgetExceeded);
            }
            let baseline = workspace::entries(&mut tx, run).await?;
            let version = workspace::add_version(
                &mut tx,
                &r,
                &format!(".browser/{session}/baseline"),
                "artifact",
                json_string(&baseline)?.as_bytes(),
                keys,
                key_id,
                now,
            )
            .await?;
            sqlx::query("INSERT INTO android_private_browser_sessions(id,run_id,owner_id,project_id,lease_id,baseline_version,last_sequence,state,revision,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'admitted',1,$8)").bind(session.to_string()).bind(run.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(lease.to_string()).bind(version).bind(sequence).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        } else {
            let changed=sqlx::query("UPDATE android_private_browser_sessions SET state='admitted',last_sequence=$1,revision=revision+1 WHERE id=$2 AND run_id=$3 AND owner_id=$4 AND project_id=$5 AND lease_id=$6 AND state='ready' AND container_nonce IS NOT NULL").bind(sequence).bind(session.to_string()).bind(run.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).bind(lease.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
            if changed != 1 {
                return Err(Error::UncertainEffect);
            }
        }
        let intent = json!({"format":1,"tool":tool,"fingerprint":tools::tool_fingerprint(run,&cp,&tool),"runner":runner,"action":action,"approval_id":req["id"],"approval_revision":req["revision"]});
        let sealed = workspace::seal_value(
            keys,
            key_id,
            &r,
            &format!("intent:{sequence}"),
            json_string(&intent)?.as_bytes(),
        )?;
        crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        let invocation = tools::invocation(&cp, &tool)?;
        sqlx::query("INSERT INTO android_private_loop_steps(run_id,sequence,invocation_id,kind,lease_id,checkpoint_sha,request_sha,intent,state,created_at) VALUES($1,$2,$3,'process',$4,$5,$6,$7,'admitted',$8)").bind(run.to_string()).bind(sequence).bind(&invocation).bind(lease.to_string()).bind(canonical_hash(&cp)).bind(canonical_hash(&intent)).bind(json_string(&sealed)?).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_private_server::save(&mut tx, &r, "running", None, None, now).await?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(BrowserAdmission {
            session,
            sequence,
            invocation,
            request_sha256: canonical_hash(&intent),
            action,
            new_session,
        })
    }
    pub async fn android_private_browser_attach(
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
        let changed=sqlx::query("UPDATE android_private_browser_sessions SET container_nonce=$1 WHERE id=$2 AND container_nonce IS NULL AND state='admitted'").bind(nonce.to_string()).bind(session.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        live(&mut tx, run, lease, session, sequence, keys).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_browser_sessions s JOIN android_private_containers c ON c.id=s.container_nonce WHERE s.id=$1 AND s.container_nonce=$2 AND c.run_id=$3 AND c.lease_id=$4 AND c.state='running'").bind(session.to_string()).bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_network_admit(
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
        if request.is_nil() || json_string(network)?.len() > 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (r, cp, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        let u = url(network["url"].as_str().ok_or_else(device_invalid)?)?;
        if !intent["action"]["allowed_origins"]
            .as_array()
            .is_some_and(|v| v.contains(&json!(u.origin().ascii_serialization())))
            || !intent["action"]["allowed_methods"]
                .as_array()
                .is_some_and(|v| v.contains(&network["method"]))
        {
            return Err(Error::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_browser_sessions s JOIN android_private_containers c ON c.id=s.container_nonce WHERE s.id=$1 AND s.container_nonce=$2 AND c.state='running'").bind(session.to_string()).bind(nonce.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_browser_requests WHERE session_id=$1",
        )
        .bind(session.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if total >= 2048 {
            return Err(Error::BudgetExceeded);
        }
        // Reserve the maximum bounded receive before the original HTTP send.
        let stored:i64=sqlx::query_scalar("SELECT CAST(COALESCE(SUM(i.size+COALESCE(v.size,0)),0) AS BIGINT) FROM android_private_browser_requests n JOIN android_private_loop_versions i ON i.id=n.intent_version LEFT JOIN android_private_loop_versions v ON v.id=n.receipt_version WHERE n.session_id=$1").bind(session.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if stored > 57 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let now = device_writer(&mut tx).await?;
        let bytes = json_string(
            &json!({"format":1,"session":session,"sequence":sequence,"nonce":nonce,"request":request,"network":network,"action_sha256":canonical_hash(&intent)}),
        )?;
        let version = workspace::add_version(
            &mut tx,
            &r,
            &format!(".browser/{session}/network/{request}/intent"),
            "artifact",
            bytes.as_bytes(),
            keys,
            key_id,
            now,
        )
        .await?;
        sqlx::query("INSERT INTO android_private_browser_requests(session_id,action_sequence,request_id,request_sha,intent_version,state,created_at) VALUES($1,$2,$3,$4,$5,'admitted',$6)").bind(session.to_string()).bind(sequence).bind(request.to_string()).bind(hash(bytes.as_bytes())).bind(version).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_network_validate(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        request: Uuid,
        network: &Value,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (r, cp, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        let saved:String=sqlx::query_scalar("SELECT intent_version FROM android_private_browser_requests WHERE session_id=$1 AND request_id=$2 AND action_sequence=$3 AND state='admitted' AND receipt_version IS NULL").bind(session.to_string()).bind(request.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let original: Value =
            serde_json::from_slice(&workspace::version_bytes(&mut tx, &r, &saved, keys).await?)
                .map_err(storage_error)?;
        if original
            != json!({"format":1,"session":session,"sequence":sequence,"nonce":nonce,"request":request,"network":network,"action_sha256":canonical_hash(&intent)})
        {
            return Err(Error::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='running'").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_network_received(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        request: Uuid,
        original_network: &Value,
        response: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        // Recovery may already have closed the original guest while this original receive is finishing.
        // Retaining an observation grants no lease, browser action, publication or HTTP replay.
        let admitted=sqlx::query("SELECT n.intent_version,n.request_sha,c.original_binding FROM android_private_browser_requests n JOIN android_private_browser_sessions s ON s.id=n.session_id JOIN android_private_containers c ON c.id=s.container_nonce WHERE n.session_id=$1 AND n.request_id=$2 AND n.action_sequence=$3 AND s.run_id=$4 AND s.lease_id=$5 AND s.container_nonce=$6 AND s.owner_id=$7 AND s.project_id=$8 AND c.run_id=$4 AND c.lease_id=$5 AND c.owner_id=$7 AND c.project_id=$8 AND n.state IN('admitted','uncertain') AND n.receipt_version IS NULL").bind(session.to_string()).bind(request.to_string()).bind(sequence).bind(run.to_string()).bind(lease.to_string()).bind(nonce.to_string()).bind(id(&r,"owner_id")?.to_string()).bind(id(&r,"project_id")?.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let request_bytes = workspace::version_bytes(
            &mut tx,
            &r,
            &admitted
                .try_get::<String, _>("intent_version")
                .map_err(storage_error)?,
            keys,
        )
        .await?;
        let network_original: Value =
            serde_json::from_slice(&request_bytes).map_err(storage_error)?;
        let action = original(&mut tx, &r, sequence, keys).await?;
        let original_step:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND lease_id=$3 AND kind='process' AND request_sha=$4").bind(run.to_string()).bind(sequence).bind(lease.to_string()).bind(canonical_hash(&action)).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let request_sha = admitted
            .try_get::<String, _>("request_sha")
            .map_err(storage_error)?;
        if original_step != 1
            || hash(&request_bytes) != request_sha
            || network_original
                != json!({"format":1,"session":session,"sequence":sequence,"nonce":nonce,"request":request,"network":original_network,"action_sha256":canonical_hash(&action)})
            || parse::<Value>(
                &admitted
                    .try_get::<String, _>("original_binding")
                    .map_err(storage_error)?,
            )? != action["runner"]
        {
            return Err(Error::Forbidden);
        }
        let bytes = json_string(
            &json!({"format":1,"run":run,"lease":lease,"session":session,"action_sequence":sequence,"nonce":nonce,"request_id":request,"original_request_sha256":request_sha,"receive":response}),
        )?;
        if bytes.len() > 6 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let version = workspace::add_version(
            &mut tx,
            &r,
            &format!(".browser/{session}/network/{request}/receipt"),
            "artifact",
            bytes.as_bytes(),
            keys,
            key_id,
            now,
        )
        .await?;
        let changed=sqlx::query("UPDATE android_private_browser_requests SET receipt_version=$1,state=$2 WHERE session_id=$3 AND request_id=$4 AND action_sequence=$5 AND state IN('admitted','uncertain') AND receipt_version IS NULL").bind(version).bind(if response["uncertain"]==true {"uncertain"}else{"received"}).bind(session.to_string()).bind(request.to_string()).bind(sequence).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_applied(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        sequence: i64,
        nonce: Uuid,
        received: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (r, mut cp, intent) = live(&mut tx, run, lease, session, sequence, keys).await?;
        let digest:String=sqlx::query_scalar("SELECT receipt_sha FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND state='received'").bind(run.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if digest != canonical_hash(received)
            || received["session_id"] != session.to_string()
            || received["nonce"] != nonce.to_string()
            || received["action_sequence"] != sequence
            || received["paused"] != true
            || !received["files"].is_array()
        {
            return Err(Error::Forbidden);
        }
        let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_browser_requests WHERE session_id=$1 AND action_sequence=$2 AND state!='received'").bind(session.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if unknown != 0 {
            return Err(Error::UncertainEffect);
        }
        let result = &received["result"];
        if result["operation"] != intent["action"]["operation"]
            || !result["dom"].is_string()
            || !result["url"].is_string()
            || json_string(result)?.len() > 2 * 1024 * 1024
        {
            return Err(Error::Forbidden);
        }
        let tool = tools::pending(&cp)?;
        let prev = canonical_hash(&cp);
        cp["pending_tools"]
            .as_array_mut()
            .ok_or_else(device_invalid)?
            .remove(0);
        cp["tool_steps"] = json!(cp["tool_steps"].as_u64().ok_or_else(device_invalid)? + 1);
        cp["messages"].as_array_mut().ok_or_else(device_invalid)?.push(json!({"role":"tool","tool_call_id":tool.id,"name":"browser","content":json_string(&json!({"session_id":session,"result":result,"files_pending_guest_close":true}))?}));
        let now = device_writer(&mut tx).await?;
        tools::set_checkpoint(&mut tx, &r, &cp, Some(&prev), keys, key_id, now).await?;
        sqlx::query("UPDATE android_private_browser_sessions SET state='ready',revision=revision+1 WHERE id=$1 AND last_sequence=$2 AND state='admitted'").bind(session.to_string()).bind(sequence).execute(&mut *tx).await.map_err(storage_error)?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_browser_closed(
        &self,
        run: Uuid,
        lease: Uuid,
        session: Uuid,
        nonce: Uuid,
        witness: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(&mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority(&mut tx, &r, &cp, now).await?;
        let s=sqlx::query("SELECT * FROM android_private_browser_sessions WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND container_nonce=$4 AND state='ready'").bind(session.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(nonce.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
        let original:String=sqlx::query_scalar("SELECT termination_witness FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='removed'").bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if parse::<Value>(&original)? != *witness {
            return Err(Error::Forbidden);
        }
        let baseline: Value = serde_json::from_slice(
            &workspace::version_bytes(
                &mut tx,
                &r,
                &s.try_get::<String, _>("baseline_version")
                    .map_err(storage_error)?,
                keys,
            )
            .await?,
        )
        .map_err(storage_error)?;
        if baseline != json!(workspace::entries(&mut tx, run).await?) {
            return Err(Error::Conflict);
        }
        let sequence: i64 = s.try_get("last_sequence").map_err(storage_error)?;
        let sealed:String=sqlx::query_scalar("SELECT receipt FROM android_private_loop_steps WHERE run_id=$1 AND sequence=$2 AND state='received'").bind(run.to_string()).bind(sequence).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let received: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            &r,
            &format!("receipt:{sequence}"),
            &parse(&sealed)?,
        )?)
        .map_err(storage_error)?;
        crate::android_private_runner::promote(&mut tx, &r, &received["files"], keys, key_id, now)
            .await?;
        sqlx::query("UPDATE android_private_browser_sessions SET state='closed',revision=revision+1 WHERE id=$1 AND state='ready'").bind(session.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tools::live_after_crypto(&mut tx, &r, &cp).await?;
        tx.commit().await.map_err(storage_error)
    }
}
