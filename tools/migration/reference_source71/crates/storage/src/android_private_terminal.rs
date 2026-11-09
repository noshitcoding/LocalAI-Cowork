//! A real actor-private terminal. Tickets, inputs and receives never grant a second physical send.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use crate::android_private_server::{authority, id, leased, row};
use crate::android_private_tools as tools;
use crate::android_private_workspace as workspace;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use ring::rand::{SecureRandom, SystemRandom};
use sqlx::{Any, Transaction, any::AnyRow};

pub struct TerminalAdmission {
    pub session: Uuid,
    pub run: Uuid,
    pub lease: Uuid,
    pub run_value: Value,
    pub bootstrap: Value,
}
async fn session(tx: &mut Transaction<'_, Any>, sid: Uuid) -> Result<AnyRow> {
    sqlx::query("SELECT * FROM android_private_terminal_sessions WHERE id=$1")
        .bind(sid.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)
}
pub(crate) async fn container_authority(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    nonce: Uuid,
    keys: &dyn KeyStore,
) -> Result<bool> {
    let invocation:String=sqlx::query_scalar("SELECT invocation_id FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3")
        .bind(nonce.to_string()).bind(id(r,"id")?.to_string()).bind(id(r,"lease_id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let Some(raw) = invocation.strip_prefix("terminal:") else {
        return Ok(false);
    };
    let sid = Uuid::parse_str(raw).map_err(|_| device_invalid())?;
    let s = session(tx, sid).await?;
    let now = device_writer(tx).await?;
    if id(&s, "run_id")? != id(r, "id")?
        || id(&s, "owner_id")? != id(r, "owner_id")?
        || id(&s, "project_id")? != id(r, "project_id")?
        || id(&s, "lease_id")? != id(r, "lease_id")?
        || s.try_get::<String, _>("state").map_err(storage_error)? != "connected"
        || s.try_get::<Option<String>, _>("container_nonce")
            .map_err(storage_error)?
            != Some(nonce.to_string())
        || s.try_get::<i64, _>("deadline").map_err(storage_error)? <= now
    {
        return Err(Error::Forbidden);
    }
    let cp = tools::checkpoint(tx, r, keys).await?;
    let fresh = device_writer(tx).await?;
    leased(tx, id(r, "id")?, id(r, "lease_id")?, fresh).await?;
    tools::loop_authority_without_terminal(tx, r, &cp, fresh).await?;
    if s.try_get::<i64, _>("deadline").map_err(storage_error)? <= fresh {
        return Err(Error::Forbidden);
    }
    Ok(true)
}
pub(crate) async fn ensure_idle(tx: &mut Transaction<'_, Any>, run: Uuid) -> Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_private_terminal_sessions WHERE run_id=$1 AND state!='closed'")
        .bind(run.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if count != 0 {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn live(
    tx: &mut Transaction<'_, Any>,
    sid: Uuid,
    keys: &dyn KeyStore,
) -> Result<(AnyRow, AnyRow, Value, i64)> {
    let now = device_writer(tx).await?;
    crate::server_keys::maintenance_fence(tx).await?;
    let s = session(tx, sid).await?;
    if s.try_get::<String, _>("state").map_err(storage_error)? != "connected"
        || s.try_get::<i64, _>("deadline").map_err(storage_error)? <= now
    {
        return Err(Error::Forbidden);
    }
    let r = leased(tx, id(&s, "run_id")?, id(&s, "lease_id")?, now).await?;
    if id(&s, "owner_id")? != id(&r, "owner_id")? || id(&s, "project_id")? != id(&r, "project_id")?
    {
        return Err(Error::Forbidden);
    }
    let cp = tools::checkpoint(tx, &r, keys).await?;
    tools::loop_authority_without_terminal(tx, &r, &cp, now).await?;
    let binding: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if let Some(nonce) = s
        .try_get::<Option<String>, _>("container_nonce")
        .map_err(storage_error)?
    {
        let original: String = sqlx::query_scalar("SELECT original_binding FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND state='running'")
            .bind(nonce).bind(id(&r,"id")?.to_string()).bind(id(&s,"lease_id")?.to_string())
            .bind(id(&s,"owner_id")?.to_string()).bind(id(&s,"project_id")?.to_string())
            .fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if parse::<Value>(&original)? != binding["spec"]["input"]["frozen_private_runner"] {
            return Err(Error::Forbidden);
        }
    }
    // Cryptographic reads may block. Re-read actual DB time and the original lease afterwards.
    let fresh = device_writer(tx).await?;
    leased(tx, id(&r, "id")?, id(&s, "lease_id")?, fresh).await?;
    tools::loop_authority_without_terminal(tx, &r, &cp, fresh).await?;
    if s.try_get::<i64, _>("deadline").map_err(storage_error)? <= fresh {
        return Err(Error::Forbidden);
    }
    Ok((s, r, cp, fresh))
}
async fn version(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    sid: Uuid,
    label: &str,
    value: &Value,
    keys: &dyn KeyStore,
    key_id: KeyId,
    now: i64,
) -> Result<String> {
    workspace::add_version(
        tx,
        r,
        &format!("terminal/{sid}/{label}.json"),
        "artifact",
        &serde_json::to_vec(value).map_err(storage_error)?,
        keys,
        key_id,
        now,
    )
    .await
}
async fn original_files(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    keys: &dyn KeyStore,
) -> Result<Vec<Value>> {
    let mut files = Vec::new();
    let mut bytes = 0usize;
    for entry in workspace::entries(tx, id(r, "id")?).await? {
        if entry["kind"] == "file" {
            let data = workspace::version_bytes(
                tx,
                r,
                entry["version_id"].as_str().ok_or_else(device_invalid)?,
                keys,
            )
            .await?;
            bytes = bytes.checked_add(data.len()).ok_or(Error::BudgetExceeded)?;
            if bytes > 16 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            files.push(
                json!({"path":entry["path"],"kind":"file","data":STANDARD.encode(data.as_slice())}),
            );
        } else {
            files.push(json!({"path":entry["path"],"kind":"directory"}));
        }
    }
    Ok(files)
}
impl Store {
    pub async fn android_private_terminal_create(
        &self,
        actor: &Principal,
        run: Uuid,
        key: &str,
        columns: u16,
        rows: u16,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Value> {
        valid_text(key, 200)?;
        if !(20..=400).contains(&columns) || !(5..=200).contains(&rows) {
            return Err(device_invalid());
        }
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if id(&r, "owner_id")? != actor.identity_id() || id(&r, "session_id")? != actor.session_id()
        {
            return Err(Error::Forbidden);
        }
        self.android_personal_permit(
            &mut tx,
            actor,
            id(&r, "project_id")?,
            ContentAction::Execute,
        )
        .await?;
        authority(&mut tx, &r, now).await?;
        let request_sha = canonical_hash(&json!({"run_id":run,"columns":columns,"rows":rows}));
        if let Some(s) = sqlx::query(
            "SELECT * FROM android_private_terminal_sessions WHERE run_id=$1 AND creation_key=$2",
        )
        .bind(run.to_string())
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if s.try_get::<String, _>("request_sha")
                .map_err(storage_error)?
                != request_sha
            {
                return Err(Error::Conflict);
            }
            let bytes = workspace::version_bytes(
                &mut tx,
                &r,
                &s.try_get::<String, _>("ticket_version")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            let receipt = serde_json::from_slice(&bytes).map_err(storage_error)?;
            let fresh = device_writer(&mut tx).await?;
            authority(&mut tx, &r, fresh).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let lease = id(&r, "lease_id")?;
        leased(&mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority_without_terminal(&mut tx, &r, &cp, now).await?;
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_terminal_sessions WHERE run_id=$1 AND state!='closed'").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if active >= 4 {
            return Err(Error::BudgetExceeded);
        }
        let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let runner = &original["spec"]["input"]["frozen_private_runner"];
        let program = runner["terminal_program"]
            .as_str()
            .ok_or(Error::Forbidden)?;
        if runner["programs"]
            .get(program)
            .and_then(Value::as_str)
            .is_none()
        {
            return Err(Error::Forbidden);
        }
        let spec: RunSpec = serde_json::from_value(cp["spec"].clone()).map_err(storage_error)?;
        if !spec
            .allowed_tools
            .iter()
            .any(|s| matches!(s.as_str(), "execute_program" | "terminal"))
        {
            return Err(Error::Forbidden);
        }
        let outstanding: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND state!='received'",
        )
        .bind(run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if outstanding != 0 {
            return Err(Error::Conflict);
        }
        let sid = Uuid::new_v4();
        let mut random = [0; 32];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let token = URL_SAFE_NO_PAD.encode(random);
        let deadline = cp["deadline"]
            .as_i64()
            .ok_or_else(device_invalid)?
            .min(now.checked_add(900_000).ok_or(Error::BudgetExceeded)?);
        if deadline <= now {
            return Err(Error::BudgetExceeded);
        }
        let expires = now
            .checked_add(60_000)
            .ok_or(Error::BudgetExceeded)?
            .min(deadline);
        let ticket = json!({"schema_version":2,"session_id":sid,"token":token,"expires_at":device_iso(expires)?,"protocol":"terminal.binary.v1"});
        let receipt = version(
            &mut tx,
            &r,
            sid,
            "original-ticket",
            &ticket,
            keys,
            key_id,
            now,
        )
        .await?;
        let entries = json!({"session_id":sid,"run_id":run,"lease_id":lease,"checkpoint_sha":canonical_hash(&cp),"entries":workspace::entries(&mut tx, run).await?});
        let baseline = version(&mut tx, &r, sid, "baseline", &entries, keys, key_id, now).await?;
        let fresh = device_writer(&mut tx).await?;
        leased(&mut tx, run, lease, fresh).await?;
        tools::loop_authority_without_terminal(&mut tx, &r, &cp, fresh).await?;
        sqlx::query("INSERT INTO android_private_terminal_sessions(id,run_id,owner_id,project_id,lease_id,creation_key,request_sha,ticket_sha,ticket_version,baseline_version,state,columns,rows,ticket_expires_at,deadline,input_sequence,output_sequence,input_bytes,output_bytes,revision,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'created',$11,$12,$13,$14,0,0,0,0,1,$15)")
            .bind(sid.to_string()).bind(run.to_string()).bind(actor.identity_id().to_string()).bind(id(&r,"project_id")?.to_string()).bind(lease.to_string()).bind(key).bind(request_sha).bind(hash(token.as_bytes())).bind(receipt).bind(baseline).bind(i64::from(columns)).bind(i64::from(rows)).bind(expires).bind(deadline).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_private_server::event(
            &mut tx,
            run,
            "terminal_session_created",
            &json!({"session_id":sid,"columns":columns,"rows":rows}),
            now,
        )
        .await?;
        workspace::bounds(&mut tx, run).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(ticket)
    }
    pub async fn android_private_terminal_connect(
        &self,
        sid: Uuid,
        ticket: &str,
        binding: &Value,
        keys: &dyn KeyStore,
    ) -> Result<TerminalAdmission> {
        if ticket.len() != 43 {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let s = session(&mut tx, sid).await?;
        if s.try_get::<String, _>("state").map_err(storage_error)? != "created"
            || s.try_get::<String, _>("ticket_sha")
                .map_err(storage_error)?
                != hash(ticket.as_bytes())
            || s.try_get::<i64, _>("ticket_expires_at")
                .map_err(storage_error)?
                <= now
        {
            return Err(Error::Forbidden);
        }
        let run = id(&s, "run_id")?;
        let lease = id(&s, "lease_id")?;
        let r = leased(&mut tx, run, lease, now).await?;
        let cp = tools::checkpoint(&mut tx, &r, keys).await?;
        tools::loop_authority_without_terminal(&mut tx, &r, &cp, now).await?;
        let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if original["spec"]["input"]["frozen_private_runner"] != *binding {
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
        if baseline["session_id"] != sid.to_string()
            || baseline["run_id"] != run.to_string()
            || baseline["lease_id"] != lease.to_string()
            || baseline["checkpoint_sha"] != canonical_hash(&cp)
            || baseline["entries"] != json!(workspace::entries(&mut tx, run).await?)
        {
            return Err(Error::Conflict);
        }
        let program_alias = binding["terminal_program"]
            .as_str()
            .ok_or(Error::Forbidden)?;
        let program = binding["programs"][program_alias]
            .as_str()
            .ok_or(Error::Forbidden)?;
        let files = original_files(&mut tx, &r, keys).await?;
        let fresh = device_writer(&mut tx).await?;
        leased(&mut tx, run, lease, fresh).await?;
        tools::loop_authority_without_terminal(&mut tx, &r, &cp, fresh).await?;
        if s.try_get::<i64, _>("ticket_expires_at")
            .map_err(storage_error)?
            <= fresh
        {
            return Err(Error::Forbidden);
        }
        let timeout = (s.try_get::<i64, _>("deadline").map_err(storage_error)? - fresh) / 1000;
        if timeout <= 0 {
            return Err(Error::Forbidden);
        }
        let bootstrap = json!({"format":1,"mode":"terminal","session_id":sid,"program":program,"arguments":[],"columns":s.try_get::<i64,_>("columns").map_err(storage_error)?,"rows":s.try_get::<i64,_>("rows").map_err(storage_error)?,"timeout_seconds":timeout,"files":files});
        if sqlx::query("UPDATE android_private_terminal_sessions SET state='connected',revision=revision+1 WHERE id=$1 AND state='created' AND revision=$2").bind(sid.to_string()).bind(s.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::Conflict); }
        tx.commit().await.map_err(storage_error)?;
        Ok(TerminalAdmission {
            session: sid,
            run,
            lease,
            run_value: original,
            bootstrap,
        })
    }
    pub async fn android_private_terminal_attached(
        &self,
        sid: Uuid,
        nonce: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (s, r, _, _) = live(&mut tx, sid, keys).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND state='running'").bind(nonce.to_string()).bind(id(&r,"id")?.to_string()).bind(id(&s,"lease_id")?.to_string()).bind(id(&s,"owner_id")?.to_string()).bind(id(&s,"project_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1
            || s.try_get::<Option<String>, _>("container_nonce")
                .map_err(storage_error)?
                .as_deref()
                != Some(nonce.to_string().as_str())
        {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_terminal_validate(
        &self,
        sid: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        live(&mut tx, sid, keys).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_private_terminal_input_admit(
        &self,
        sid: Uuid,
        request: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<i64> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (s, r, _, now) = live(&mut tx, sid, keys).await?;
        let fields = request.as_object().ok_or_else(device_invalid)?;
        let allowed: &[&str] = match request["operation"].as_str() {
            Some("input") => &["operation", "data"],
            Some("resize") => &["operation", "columns", "rows"],
            Some("close") => &["operation"],
            _ => return Err(device_invalid()),
        };
        if fields.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err(device_invalid());
        }
        let size = match request["operation"].as_str() {
            Some("input") => {
                let raw = STANDARD
                    .decode(request["data"].as_str().ok_or_else(device_invalid)?)
                    .map_err(|_| device_invalid())?;
                if raw.is_empty() || raw.len() > 65536 {
                    return Err(Error::BudgetExceeded);
                }
                raw.len() as i64
            }
            Some("resize")
                if request["columns"]
                    .as_u64()
                    .is_some_and(|v| (20..=400).contains(&v))
                    && request["rows"]
                        .as_u64()
                        .is_some_and(|v| (5..=200).contains(&v)) =>
            {
                0
            }
            Some("close") => 0,
            _ => return Err(device_invalid()),
        };
        let uncertain:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_terminal_inputs WHERE session_id=$1 AND state!='received'").bind(sid.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let sequence = s
            .try_get::<i64, _>("input_sequence")
            .map_err(storage_error)?
            .checked_add(1)
            .ok_or(Error::BudgetExceeded)?;
        let total = s
            .try_get::<i64, _>("input_bytes")
            .map_err(storage_error)?
            .checked_add(size)
            .ok_or(Error::BudgetExceeded)?;
        if uncertain != 0 || sequence > 4000 || total > 32 * 1024 * 1024 {
            return Err(Error::UncertainEffect);
        }
        let intent = version(
            &mut tx,
            &r,
            sid,
            &format!("input-{sequence}"),
            &json!({"session_id":sid,"sequence":sequence,"request":request}),
            keys,
            key_id,
            now,
        )
        .await?;
        live(&mut tx, sid, keys).await?;
        sqlx::query("INSERT INTO android_private_terminal_inputs(session_id,sequence,request_sha,intent_version,state) VALUES($1,$2,$3,$4,'admitted')").bind(sid.to_string()).bind(sequence).bind(canonical_hash(request)).bind(intent).execute(&mut *tx).await.map_err(storage_error)?;
        if sqlx::query("UPDATE android_private_terminal_sessions SET input_sequence=$1,input_bytes=$2,revision=revision+1 WHERE id=$3 AND revision=$4 AND state='connected'").bind(sequence).bind(total).bind(sid.to_string()).bind(s.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        workspace::bounds(&mut tx, id(&r, "id")?).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(sequence)
    }
    /// Original receive evidence remains retainable after lease/session loss; it grants no further IO.
    pub async fn android_private_terminal_received(
        &self,
        sid: Uuid,
        nonce: Uuid,
        frame: &Value,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let s = session(&mut tx, sid).await?;
        let r = row(&mut tx, id(&s, "run_id")?).await?;
        if frame["session_id"] != sid.to_string()
            || frame["nonce"] != nonce.to_string()
            || s.try_get::<Option<String>, _>("container_nonce")
                .map_err(storage_error)?
                != Some(nonce.to_string())
            || id(&s, "owner_id")? != id(&r, "owner_id")?
            || id(&s, "project_id")? != id(&r, "project_id")?
        {
            return Err(Error::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5").bind(nonce.to_string()).bind(id(&r,"id")?.to_string()).bind(id(&s,"lease_id")?.to_string()).bind(id(&s,"owner_id")?.to_string()).bind(id(&s,"project_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        let sequence = frame["sequence"]
            .as_i64()
            .filter(|n| *n > 0 && *n <= 4000)
            .ok_or_else(device_invalid)?;
        if frame["kind"] == "terminal_output" {
            let bytes = STANDARD
                .decode(
                    frame["data"]
                        .as_str()
                        .filter(|s| s.len() <= 45000)
                        .ok_or_else(device_invalid)?,
                )
                .map_err(|_| device_invalid())?;
            if bytes.is_empty() || bytes.len() > 32768 || frame["sha256"] != hash(&bytes) {
                return Err(Error::Forbidden);
            }
            if let Some(old)=sqlx::query_scalar::<_,String>("SELECT original_sha FROM android_private_terminal_outputs WHERE session_id=$1 AND sequence=$2").bind(sid.to_string()).bind(sequence).fetch_optional(&mut *tx).await.map_err(storage_error)?{if old!=canonical_hash(frame){return Err(Error::Conflict)}tx.commit().await.map_err(storage_error)?;return Ok(())}
            let total = s
                .try_get::<i64, _>("output_bytes")
                .map_err(storage_error)?
                .checked_add(bytes.len() as i64)
                .ok_or(Error::BudgetExceeded)?;
            if sequence
                != s.try_get::<i64, _>("output_sequence")
                    .map_err(storage_error)?
                    + 1
                || total > 32 * 1024 * 1024
            {
                return Err(Error::BudgetExceeded);
            }
            let v = version(
                &mut tx,
                &r,
                sid,
                &format!("output-{sequence}"),
                frame,
                keys,
                key_id,
                now,
            )
            .await?;
            sqlx::query("INSERT INTO android_private_terminal_outputs(session_id,sequence,original_sha,version_id) VALUES($1,$2,$3,$4)").bind(sid.to_string()).bind(sequence).bind(canonical_hash(frame)).bind(v).execute(&mut *tx).await.map_err(storage_error)?;
            sqlx::query("UPDATE android_private_terminal_sessions SET output_sequence=$1,output_bytes=$2,revision=revision+1 WHERE id=$3").bind(sequence).bind(total).bind(sid.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        } else if frame["kind"] == "terminal_input" {
            let input = sqlx::query(
                "SELECT * FROM android_private_terminal_inputs WHERE session_id=$1 AND sequence=$2",
            )
            .bind(sid.to_string())
            .bind(sequence)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Forbidden)?;
            if let Some(v) = input
                .try_get::<Option<String>, _>("receipt_version")
                .map_err(storage_error)?
            {
                let previous: Value =
                    serde_json::from_slice(&workspace::version_bytes(&mut tx, &r, &v, keys).await?)
                        .map_err(storage_error)?;
                if previous != *frame {
                    return Err(Error::Conflict);
                }
                tx.commit().await.map_err(storage_error)?;
                return Ok(());
            }
            let intent: Value = serde_json::from_slice(
                &workspace::version_bytes(
                    &mut tx,
                    &r,
                    &input
                        .try_get::<String, _>("intent_version")
                        .map_err(storage_error)?,
                    keys,
                )
                .await?,
            )
            .map_err(storage_error)?;
            if intent["session_id"] != sid.to_string() || intent["sequence"] != sequence {
                return Err(Error::Forbidden);
            }
            let request = &intent["request"];
            if canonical_hash(request)
                != input
                    .try_get::<String, _>("request_sha")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            if frame["operation"] != request["operation"] || frame["complete"].as_bool().is_none() {
                return Err(Error::Forbidden);
            }
            let expected = if request["operation"] == "input" {
                STANDARD
                    .decode(request["data"].as_str().ok_or_else(device_invalid)?)
                    .map_err(|_| device_invalid())?
                    .len()
            } else {
                0
            };
            let written = frame["written_bytes"].as_u64().ok_or_else(device_invalid)? as usize;
            if written > expected || frame["complete"] == true && written != expected {
                return Err(Error::Forbidden);
            }
            let v = version(
                &mut tx,
                &r,
                sid,
                &format!("input-received-{sequence}"),
                frame,
                keys,
                key_id,
                now,
            )
            .await?;
            sqlx::query("UPDATE android_private_terminal_inputs SET receipt_version=$1,state=$2 WHERE session_id=$3 AND sequence=$4 AND receipt_version IS NULL").bind(v).bind(if frame["complete"]==true{"received"}else{"uncertain"}).bind(sid.to_string()).bind(sequence).execute(&mut *tx).await.map_err(storage_error)?;
        } else {
            return Err(device_invalid());
        }
        workspace::bounds(&mut tx, id(&r, "id")?).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_terminal_close(
        &self,
        sid: Uuid,
        nonce: Uuid,
        closed: Option<&Value>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let s = session(&mut tx, sid).await?;
        let r = row(&mut tx, id(&s, "run_id")?).await?;
        if s.try_get::<Option<String>, _>("container_nonce")
            .map_err(storage_error)?
            != Some(nonce.to_string())
        {
            return Err(Error::Forbidden);
        }
        let removed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND state='removed' AND termination_witness IS NOT NULL").bind(nonce.to_string()).bind(id(&r,"id")?.to_string()).bind(id(&s,"lease_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if removed != 1 {
            return Err(Error::UncertainEffect);
        }
        let uncertain:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_terminal_inputs WHERE session_id=$1 AND state!='received'").bind(sid.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let complete = closed.is_some_and(|v| {
            v["kind"] == "terminal_closed"
                && v["session_id"] == sid.to_string()
                && v["nonce"] == nonce.to_string()
                && v["descendants_ended"] == true
                && v["output_sequence"].as_i64() == s.try_get::<i64, _>("output_sequence").ok()
                && v["input_sequence"].as_i64() == s.try_get::<i64, _>("input_sequence").ok()
                && v["output_bytes"].as_i64() == s.try_get::<i64, _>("output_bytes").ok()
                && v["input_bytes"].as_i64() == s.try_get::<i64, _>("input_bytes").ok()
                && v["reason"] != "partial_input"
                && v["exit_code"].as_i64().is_some()
        });
        let receipt = json!({"format":1,"session_id":sid,"container_nonce":nonce,"original_closed":closed,"original_physical_removed":true,"input_uncertain":uncertain!=0,"execution_adopted":false});
        if s.try_get::<Option<String>, _>("receipt_version")
            .map_err(storage_error)?
            .is_some()
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        let v = version(
            &mut tx,
            &r,
            sid,
            "original-closure",
            &receipt,
            keys,
            key_id,
            now,
        )
        .await?;
        sqlx::query("UPDATE android_private_terminal_sessions SET state='closing',receipt_version=$1,revision=revision+1 WHERE id=$2 AND receipt_version IS NULL").bind(&v).bind(sid.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        // The original receive survives any following authority, quota or baseline failure.
        tx.commit().await.map_err(storage_error)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let s = session(&mut tx, sid).await?;
        let r = row(&mut tx, id(&s, "run_id")?).await?;
        // Publishing failures cannot discard or permanently strand the confirmed physical closure.
        let publishing:Result<bool>=async {
        let mut promoted=false;
        if complete
            && uncertain == 0
            && leased(&mut tx, id(&r, "id")?, id(&s, "lease_id")?, now)
                .await
                .is_ok()
        {
            let cp = tools::checkpoint(&mut tx, &r, keys).await?;
            tools::loop_authority_without_terminal(&mut tx, &r, &cp, now).await?;
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
            if baseline["session_id"]!=sid.to_string()||baseline["run_id"]!=id(&r,"id")?.to_string()||baseline["lease_id"]!=id(&s,"lease_id")?.to_string(){return Err(Error::Forbidden)}
            if baseline["checkpoint_sha"]==canonical_hash(&cp)&&baseline["entries"] == json!(workspace::entries(&mut tx, id(&r, "id")?).await?) {
                let fresh = device_writer(&mut tx).await?;
                leased(&mut tx, id(&r, "id")?, id(&s, "lease_id")?, fresh).await?;
                tools::loop_authority_without_terminal(&mut tx, &r, &cp, fresh).await?;
                crate::android_private_runner::promote(
                    &mut tx,
                    &r,
                    &closed.ok_or(Error::UncertainEffect)?["files"],
                    keys,
                    key_id,
                    fresh,
                )
                .await?;
                promoted = true;
            }
        }
        let publication=json!({"format":1,"session_id":sid,"container_nonce":nonce,"original_receipt_sha256":canonical_hash(&receipt),"files_promoted":promoted,"execution_adopted":false});
        let publication_version=version(&mut tx,&r,sid,"original-publication",&publication,keys,key_id,now).await?;
        if sqlx::query("UPDATE android_private_terminal_sessions SET publication_version=$1,revision=revision+1 WHERE id=$2 AND state='closing' AND receipt_version=$3 AND publication_version IS NULL").bind(publication_version).bind(sid.to_string()).bind(&v).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        tx.commit().await.map_err(storage_error)?;
        Ok(promoted)
        }.await;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let promoted = matches!(&publishing, Ok(true));
        sqlx::query("UPDATE android_private_terminal_sessions SET state=$1,revision=revision+1 WHERE id=$2 AND state='closing' AND receipt_version=$3").bind(if complete&&uncertain==0{"closed"}else{"uncertain"}).bind(sid.to_string()).bind(v).execute(&mut *tx).await.map_err(storage_error)?;
        crate::android_private_server::event(&mut tx,id(&r,"id")?,"terminal_session_closed",&json!({"session_id":sid,"files_promoted":promoted,"publication_incomplete":publishing.is_err(),"original_closed":complete,"input_uncertain":uncertain!=0}),now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_terminal_busy(&self, run: Uuid, lease: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        leased(&mut tx, run, lease, now).await?;
        sqlx::query("UPDATE android_private_terminal_sessions SET state='closed',revision=revision+1 WHERE run_id=$1 AND state IN('created','connected') AND ticket_expires_at<=$2 AND container_nonce IS NULL").bind(run.to_string()).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_terminal_sessions WHERE run_id=$1 AND state!='closed'").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(count != 0)
    }
    pub async fn android_private_terminal_recover(&self, keys: &dyn KeyStore) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        sqlx::query("UPDATE android_private_terminal_sessions SET state='closed',revision=revision+1 WHERE state IN('created','connected') AND ticket_expires_at<=$1 AND container_nonce IS NULL").bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        // A removed container permits ownership closure only; it never supplies missing input/output or files.
        sqlx::query("UPDATE android_private_terminal_sessions SET state='uncertain',revision=revision+1 WHERE state='connected' AND container_nonce IN(SELECT id FROM android_private_containers WHERE state='removed' AND termination_witness IS NOT NULL)").execute(&mut *tx).await.map_err(storage_error)?;
        let closing=sqlx::query("SELECT s.* FROM android_private_terminal_sessions s JOIN android_private_server_runs r ON r.id=s.run_id JOIN android_private_containers c ON c.id=s.container_nonce WHERE s.state='closing' AND s.receipt_version IS NOT NULL AND c.state='removed' AND c.termination_witness IS NOT NULL AND (r.lease_until<=$1 OR r.state NOT IN('running','waiting_for_input','waiting_for_approval'))").bind(now).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for s in closing {
            let sid = id(&s, "id")?;
            let r = row(&mut tx, id(&s, "run_id")?).await?;
            let v = s
                .try_get::<String, _>("receipt_version")
                .map_err(storage_error)?;
            let receipt: Value =
                serde_json::from_slice(&workspace::version_bytes(&mut tx, &r, &v, keys).await?)
                    .map_err(storage_error)?;
            let nonce = s
                .try_get::<String, _>("container_nonce")
                .map_err(storage_error)?;
            if receipt["session_id"] != sid.to_string()
                || receipt["container_nonce"] != nonce
                || receipt["original_physical_removed"] != true
                || receipt["execution_adopted"] != false
            {
                return Err(Error::Forbidden);
            }
            let closed = &receipt["original_closed"];
            let complete = closed["session_id"] == sid.to_string()
                && closed["nonce"] == nonce
                && closed["descendants_ended"] == true
                && closed["exit_code"].as_i64().is_some()
                && closed["input_sequence"].as_i64() == s.try_get::<i64, _>("input_sequence").ok()
                && closed["output_sequence"].as_i64()
                    == s.try_get::<i64, _>("output_sequence").ok()
                && closed["input_bytes"].as_i64() == s.try_get::<i64, _>("input_bytes").ok()
                && closed["output_bytes"].as_i64() == s.try_get::<i64, _>("output_bytes").ok()
                && receipt["input_uncertain"] == false
                && closed["reason"] != "partial_input";
            sqlx::query("UPDATE android_private_terminal_sessions SET state=$1,revision=revision+1 WHERE id=$2 AND state='closing' AND receipt_version=$3").bind(if complete{"closed"}else{"uncertain"}).bind(sid.to_string()).bind(v).execute(&mut *tx).await.map_err(storage_error)?;
            crate::android_private_server::event(&mut tx,id(&r,"id")?,"terminal_closure_reconciled",&json!({"session_id":sid,"original_complete":complete,"publication_replayed":false,"execution_adopted":false}),now).await?;
        }
        tx.commit().await.map_err(storage_error)
    }
    /// This is only observation of original guest bytes, including an unfinished JSON frame.
    pub async fn android_private_terminal_partial_received(
        &self,
        sid: Uuid,
        nonce: Uuid,
        bytes: &[u8],
        complete_frame: bool,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > 32 * 1024 * 1024 + 8192 {
            return Err(Error::BudgetExceeded);
        }
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let s = session(&mut tx, sid).await?;
        let r = row(&mut tx, id(&s, "run_id")?).await?;
        if s.try_get::<Option<String>, _>("container_nonce")
            .map_err(storage_error)?
            != Some(nonce.to_string())
            || id(&s, "owner_id")? != id(&r, "owner_id")?
            || id(&s, "project_id")? != id(&r, "project_id")?
        {
            return Err(Error::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3").bind(nonce.to_string()).bind(id(&s,"run_id")?.to_string()).bind(id(&s,"lease_id")?.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 1 {
            return Err(Error::Forbidden);
        }
        // Leave room for the JSON envelope within the existing 32 MiB version limit.
        let retained = &bytes[..bytes.len().min(24 * 1024 * 1024 - 64 * 1024)];
        let value = json!({"format":1,"session_id":sid,"run_id":id(&s,"run_id")?,"lease_id":id(&s,"lease_id")?,"container_nonce":nonce,"observed_bytes":bytes.len(),"original_sha256":hash(bytes),"prefix_only":retained.len()!=bytes.len(),"frame_complete":complete_frame,"data":STANDARD.encode(retained),"uncertain":true,"execution_adopted":false});
        let v = version(
            &mut tx,
            &r,
            sid,
            &format!("partial-{}", Uuid::new_v4()),
            &value,
            keys,
            key_id,
            now,
        )
        .await?;
        crate::android_private_server::event(&mut tx,id(&r,"id")?,"terminal_original_control_received",&json!({"session_id":sid,"version_id":v,"observed_bytes":bytes.len(),"frame_complete":complete_frame,"physical_closure_pending":true}),now).await?;
        workspace::bounds(&mut tx, id(&r, "id")?).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_private_terminal_history(
        &self,
        actor: &Principal,
        run: Uuid,
        keys: &dyn KeyStore,
        session_filter: Option<Uuid>,
        after: i64,
    ) -> Result<Value> {
        if after < 0 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        if id(&r, "owner_id")? != actor.identity_id() {
            return Err(Error::Forbidden);
        }
        self.android_personal_permit(&mut tx, actor, id(&r, "project_id")?, ContentAction::Read)
            .await?;
        let mut items = Vec::new();
        let sessions=sqlx::query("SELECT * FROM android_private_terminal_sessions WHERE run_id=$1 ORDER BY created_at,id LIMIT 101").bind(run.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if sessions.len() > 100 {
            return Err(Error::BudgetExceeded);
        }
        for s in sessions {
            let sid = id(&s, "id")?;
            if session_filter.is_some_and(|v| v != sid) {
                continue;
            }
            let mut value = json!({"schema_version":2,"session_id":sid,"run_id":run,"state":s.try_get::<String,_>("state").map_err(storage_error)?,"columns":s.try_get::<i64,_>("columns").map_err(storage_error)?,"rows":s.try_get::<i64,_>("rows").map_err(storage_error)?,"input_bytes":s.try_get::<i64,_>("input_bytes").map_err(storage_error)?,"output_bytes":s.try_get::<i64,_>("output_bytes").map_err(storage_error)?,"created_at":device_iso(s.try_get::<i64,_>("created_at").map_err(storage_error)?)?,"frames":[]});
            if session_filter.is_some() {
                let prefix = format!("terminal/{sid}/partial-%");
                let observations=sqlx::query("SELECT id,digest,size,created_at FROM android_private_loop_versions WHERE run_id=$1 AND kind='artifact' AND path LIKE $2 ORDER BY created_at,id LIMIT 100").bind(run.to_string()).bind(prefix).fetch_all(&mut *tx).await.map_err(storage_error)?;
                value["observations"]=json!(observations.into_iter().map(|o|Ok(json!({"version_id":o.try_get::<String,_>("id").map_err(storage_error)?,"sha256":o.try_get::<String,_>("digest").map_err(storage_error)?,"size":o.try_get::<i64,_>("size").map_err(storage_error)?,"created_at":device_iso(o.try_get::<i64,_>("created_at").map_err(storage_error)?)?}))).collect::<Result<Vec<Value>>>()?);
                let frames=sqlx::query("SELECT sequence,version_id FROM android_private_terminal_outputs WHERE session_id=$1 AND sequence>$2 ORDER BY sequence LIMIT 100").bind(sid.to_string()).bind(after).fetch_all(&mut *tx).await.map_err(storage_error)?;
                let mut values = Vec::new();
                for f in frames {
                    values.push(
                        serde_json::from_slice::<Value>(
                            &workspace::version_bytes(
                                &mut tx,
                                &r,
                                &f.try_get::<String, _>("version_id")
                                    .map_err(storage_error)?,
                                keys,
                            )
                            .await?,
                        )
                        .map_err(storage_error)?,
                    );
                }
                value["frames"] = json!(values);
                if let Some(version) = s
                    .try_get::<Option<String>, _>("receipt_version")
                    .map_err(storage_error)?
                {
                    value["original_receipt"] = serde_json::from_slice(
                        &workspace::version_bytes(&mut tx, &r, &version, keys).await?,
                    )
                    .map_err(storage_error)?;
                }
                if let Some(publication) = s
                    .try_get::<Option<String>, _>("publication_version")
                    .map_err(storage_error)?
                {
                    value["publication"] = serde_json::from_slice::<Value>(
                        &workspace::version_bytes(&mut tx, &r, &publication, keys).await?,
                    )
                    .map_err(storage_error)?;
                }
            }
            items.push(value);
        }
        let _fresh = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, id(&r, "project_id")?, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"items":items,"after":after,"observed_at":device_iso(now)?}))
    }
    pub async fn android_private_terminal_observation(
        &self,
        actor: &Principal,
        run: Uuid,
        sid: Uuid,
        version_id: &str,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = row(&mut tx, run).await?;
        let s = session(&mut tx, sid).await?;
        if id(&r, "owner_id")? != actor.identity_id()
            || id(&s, "run_id")? != run
            || id(&s, "owner_id")? != actor.identity_id()
        {
            return Err(Error::Forbidden);
        }
        self.android_personal_permit(&mut tx, actor, id(&r, "project_id")?, ContentAction::Read)
            .await?;
        let path:String=sqlx::query_scalar("SELECT path FROM android_private_loop_versions WHERE id=$1 AND run_id=$2 AND kind='artifact'").bind(version_id).bind(run.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if !path.starts_with(&format!("terminal/{sid}/partial-")) {
            return Err(Error::Forbidden);
        }
        let value: Value =
            serde_json::from_slice(&workspace::version_bytes(&mut tx, &r, version_id, keys).await?)
                .map_err(storage_error)?;
        if value["format"] != 1
            || value["session_id"] != sid.to_string()
            || value["run_id"] != run.to_string()
            || value["lease_id"] != id(&s, "lease_id")?.to_string()
            || value["execution_adopted"] != false
        {
            return Err(Error::Forbidden);
        }
        self.android_personal_permit(&mut tx, actor, id(&r, "project_id")?, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
