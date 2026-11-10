//! Encrypted original lease/permit and immutable reconnect batches. No execution is replayed.
use super::*;
use crate::enterprise_policy::{digest, Pin};
const SCHEMA:&str="CREATE TABLE IF NOT EXISTS daemon_enterprise_offline(run_id TEXT PRIMARY KEY,permit_id TEXT UNIQUE,encrypted_original TEXT NOT NULL);CREATE TABLE IF NOT EXISTS daemon_enterprise_offline_batches(request_id TEXT PRIMARY KEY,run_id TEXT NOT NULL,sequence_end INTEGER NOT NULL,finished INTEGER NOT NULL,encrypted_body TEXT NOT NULL,encrypted_receipt TEXT,acked INTEGER NOT NULL DEFAULT 0);CREATE TABLE IF NOT EXISTS daemon_enterprise_original_events(run_id TEXT NOT NULL,sequence INTEGER NOT NULL,encrypted_original TEXT NOT NULL,PRIMARY KEY(run_id,sequence));";
#[derive(Serialize, Deserialize)]
struct Journal {
    format: u16,
    server_url: String,
    credential_sha256: String,
    lease: RunLease,
    request_id: Uuid,
    prepared_at: DateTime<Utc>,
    native_deadline: DateTime<Utc>,
    permit: Option<Value>,
    #[serde(default)]
    settlement: Option<Value>,
    #[serde(default)]
    completion: Option<Value>,
}
#[derive(Serialize, Deserialize)]
struct Event {
    sequence: i64,
    event_id: Uuid,
    kind: RunEventKind,
    payload: Value,
    created_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
struct Batch {
    request_id: Uuid,
    events: Vec<Event>,
    finished: bool,
    result: Option<Value>,
}
fn seal(daemon: &Daemon, v: &impl Serialize) -> Result<String> {
    encrypt_secret_payload(&serde_json::to_vec(v)?, &daemon.config.model_secret_key)
}
fn open<T: serde::de::DeserializeOwned>(daemon: &Daemon, s: &str) -> Result<T> {
    Ok(serde_json::from_slice(&decrypt_secret_payload(
        s,
        &daemon.config.model_secret_key,
    )?)?)
}
fn journal(daemon: &Daemon, db: &Connection, id: Uuid) -> Result<Option<Journal>> {
    db.execute_batch(SCHEMA)?;
    let s: Option<String> = db
        .query_row(
            "SELECT encrypted_original FROM daemon_enterprise_offline WHERE run_id=?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    s.map(|s| open(daemon, &s)).transpose()
}
pub(super) fn authorized(
    daemon: &Daemon,
    db: &Connection,
    spec: &RunSpec,
    pin: &Pin,
    now: DateTime<Utc>,
) -> Result<bool> {
    let Some(j) = journal(daemon, db, spec.id)? else {
        return Ok(false);
    };
    let Some(p) = &j.permit else { return Ok(false) };
    let until: DateTime<Utc> = serde_json::from_value(p["valid_until"].clone())?;
    Ok(j.format == 1
        && j.lease.run.spec.id == spec.id
        && digest(&j.lease.run.spec)? == digest(spec)?
        && p["grant_id"] == json!(pin.grant_id)
        && p["grant_revision"] == json!(pin.revision)
        && p["device_id"] == json!(daemon.config.device_id)
        && p["original_spec_sha256"] == json!(digest(spec)?)
        && now < until
        && now < j.native_deadline)
}
pub(super) async fn prepare(daemon: &Daemon, params: Value) -> Result<Value> {
    let lease: RunLease = serde_json::from_value(params["lease"].clone())?;
    let pin = Pin::from_input(&lease.run.spec.input)?.context("enterprise original pin missing")?;
    let c = crate::enterprise_local::custody(daemon, &pin, &lease.run.spec).await?;
    if c.policy.offline_seconds == 0 {
        bail!("original policy forbids offline execution");
    }
    let server = params["server_url"]
        .as_str()
        .context("actual server missing")?
        .trim_end_matches('/');
    let credential = params["credential_sha256"]
        .as_str()
        .filter(|s| crate::enterprise_policy::sha(s))
        .context("actual source credential digest missing")?;
    if c.server_url != server || lease.lease_token.is_nil() {
        bail!("offline source lease differs from native custody")
    };
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let at = crate::enterprise_local::clock(&db)?;
    if let Some(old) = journal(daemon, &db, lease.run.spec.id)? {
        if old.server_url != server
            || old.credential_sha256 != credential
            || old.lease.lease_token != lease.lease_token
            || digest(&old.lease.run.spec)? != digest(&lease.run.spec)?
        {
            bail!("different original offline lease or credential")
        };
        return Ok(
            json!({"request_id":old.request_id,"lease_token":old.lease.lease_token,"permit":old.permit}),
        );
    }
    let j = Journal {
        format: 1,
        server_url: server.into(),
        credential_sha256: credential.into(),
        lease,
        request_id: Uuid::new_v4(),
        prepared_at: at,
        native_deadline: at + chrono::Duration::seconds(i64::from(c.policy.offline_seconds)),
        permit: None,
        settlement: None,
        completion: None,
    };
    db.execute("INSERT INTO daemon_enterprise_offline(run_id,permit_id,encrypted_original) VALUES(?1,NULL,?2)",params![j.lease.run.spec.id.to_string(),seal(daemon,&j)?])?;
    Ok(json!({"request_id":j.request_id,"lease_token":j.lease.lease_token,"permit":null}))
}
pub(super) async fn accept(daemon: &Daemon, params: Value) -> Result<Value> {
    let run = run_id_param(&params)?;
    let original = params["permit"].clone();
    let db = daemon.database.lock().await;
    let mut j = journal(daemon, &db, run)?.context("original offline request missing")?;
    let pin = Pin::from_input(&j.lease.run.spec.input)?.context("original pin missing")?;
    let id: Uuid = serde_json::from_value(original["permit_id"].clone())?;
    let until: DateTime<Utc> = serde_json::from_value(original["valid_until"].clone())?;
    if original["format"] != 1
        || id.is_nil()
        || original["request_id"] != json!(j.request_id)
        || original["run_id"] != json!(run)
        || original["device_id"] != json!(daemon.config.device_id)
        || original["grant_id"] != json!(pin.grant_id)
        || original["grant_revision"] != json!(pin.revision)
        || original["original_spec_sha256"] != json!(digest(&j.lease.run.spec)?)
        || original["original_lease_token"] != json!(j.lease.lease_token)
        || original["max_events"] != json!(pin.max_offline_events)
        || until <= j.prepared_at
    {
        bail!("original server offline receipt differs from native request")
    };
    if let Some(old) = &j.permit {
        if *old != original {
            bail!("different immutable original offline receipt")
        }
    }
    j.permit = Some(original.clone());
    db.execute(
        "UPDATE daemon_enterprise_offline SET permit_id=?1,encrypted_original=?2 WHERE run_id=?3",
        params![id.to_string(), seal(daemon, &j)?, run.to_string()],
    )?;
    Ok(original)
}
// Called inside the existing local event transaction. Preserve the original before checking limits.
pub(super) fn event(daemon: &Daemon, db: &Connection, event: &RunEvent) -> Result<()> {
    let Some(j) = journal(daemon, db, event.run_id)? else {
        return Ok(());
    };
    let Some(p) = j.permit else { return Ok(()) };
    db.execute("INSERT INTO daemon_enterprise_original_events(run_id,sequence,encrypted_original) VALUES(?1,?2,?3)",params![event.run_id.to_string(),event.sequence,seal(daemon,event)?])?;
    if event.sequence > p["max_events"].as_i64().unwrap_or(0)
        || serde_json::to_vec(&event.payload)?.len() > 128 * 1024
    {
        bail!("original offline event bound exceeded; retained locally, execution held");
    }
    Ok(())
}
pub(super) async fn pending(daemon: &Daemon, params: Value) -> Result<Value> {
    let server = params["server_url"].as_str().context("server missing")?;
    let credential = params["credential_sha256"]
        .as_str()
        .context("credential digest missing")?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let mut stmt = db.prepare(
        "SELECT encrypted_original FROM daemon_enterprise_offline ORDER BY run_id LIMIT 1000",
    )?;
    let sealed = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for s in sealed {
        let j: Journal = open(daemon, &s)?;
        if j.server_url == server.trim_end_matches('/') && j.credential_sha256 == credential {
            let finished:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM daemon_enterprise_offline_batches WHERE run_id=?1 AND finished=1 AND acked=1)",[j.lease.run.spec.id.to_string()],|r|r.get(0))?;
            if !finished || j.settlement.is_none() {
                out.push(json!({"lease":j.lease,"permit":j.permit,"request_id":j.request_id,"completion":j.completion}))
            }
        }
    }
    Ok(json!(out))
}
pub(super) async fn batch(daemon: &Daemon, params: Value) -> Result<Value> {
    let run = run_id_param(&params)?;
    let db = daemon.database.lock().await;
    let j = journal(daemon, &db, run)?.context("original offline custody missing")?;
    let p = j.permit.context("actual offline permit missing")?;
    if let Some(s)=db.query_row("SELECT encrypted_body FROM daemon_enterprise_offline_batches WHERE run_id=?1 AND acked=0 ORDER BY sequence_end,request_id LIMIT 1",[run.to_string()],|r|r.get::<_,String>(0)).optional()? {return Ok(json!({"permit_id":p["permit_id"],"body":open::<Batch>(daemon,&s)?}))}
    let finished:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM daemon_enterprise_offline_batches WHERE run_id=?1 AND finished=1 AND acked=1)",[run.to_string()],|r|r.get(0))?;
    if finished {
        return Ok(Value::Null);
    };
    let cursor:i64=db.query_row("SELECT COALESCE(MAX(sequence_end),0) FROM daemon_enterprise_offline_batches WHERE run_id=?1 AND acked=1",[run.to_string()],|r|r.get(0))?;
    let record: RunRecord = serde_json::from_str(&db.query_row::<String, _, _>(
        "SELECT record_json FROM daemon_runs WHERE id=?1",
        [run.to_string()],
        |r| r.get(0),
    )?)?;
    let mut stmt=db.prepare("SELECT event_json FROM daemon_run_events WHERE run_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 1000")?;
    let rows = stmt
        .query_map(params![run.to_string(), cursor], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let until: DateTime<Utc> = serde_json::from_value(p["valid_until"].clone())?;
    let mut b = Batch {
        request_id: Uuid::new_v4(),
        events: Vec::new(),
        finished: false,
        result: None,
    };
    let mut end = cursor;
    let mut blocked = false;
    for s in rows {
        let e: RunEvent = serde_json::from_str(&s)?;
        db.execute("INSERT OR IGNORE INTO daemon_enterprise_original_events(run_id,sequence,encrypted_original) VALUES(?1,?2,?3)",params![run.to_string(),e.sequence,seal(daemon,&e)?])?;
        if e.sequence > p["max_events"].as_i64().unwrap_or(0)
            || e.created_at > until
            || serde_json::to_vec(&e.payload)?.len() > 128 * 1024
        {
            blocked = true;
            break;
        }
        let next = Event {
            sequence: e.sequence,
            event_id: e.event_id,
            kind: e.kind,
            payload: e.payload,
            created_at: e.created_at,
        };
        b.events.push(next);
        if serde_json::to_vec(&b)?.len() > 1900 * 1024 {
            b.events.pop();
            break;
        }
        end = e.sequence;
    }
    let remaining: i64 = db.query_row(
        "SELECT COUNT(*) FROM daemon_run_events WHERE run_id=?1 AND sequence>?2",
        params![run.to_string(), end],
        |r| r.get(0),
    )?;
    if (record.state.is_terminal() || record.state == RunState::Interrupted)
        && (remaining == 0 || blocked)
    {
        b.finished = true;
        b.result = Some(
            json!({"original_record":record,"local_original_events_retained":true,"event_export_blocked":blocked,"execution_replayed":false,"physical_stop_confirmed":false}),
        );
    }
    if b.events.is_empty() && !b.finished {
        return Ok(Value::Null);
    };
    if serde_json::to_vec(&b)?.len() > 2 * 1024 * 1024 {
        bail!("original terminal result exceeds bounded reconnect body; retained encrypted locally")
    }
    db.execute("INSERT INTO daemon_enterprise_offline_batches(request_id,run_id,sequence_end,finished,encrypted_body) VALUES(?1,?2,?3,?4,?5)",params![b.request_id.to_string(),run.to_string(),end,b.finished,seal(daemon,&b)?])?;
    Ok(json!({"permit_id":p["permit_id"],"body":b}))
}
pub(super) async fn ack(daemon: &Daemon, params: Value) -> Result<Value> {
    let run = run_id_param(&params)?;
    let receipt = &params["receipt"];
    let id: Uuid = serde_json::from_value(receipt["request_id"].clone())?;
    let db = daemon.database.lock().await;
    let j = journal(daemon, &db, run)?.context("original offline journal missing")?;
    let p = j.permit.context("permit missing")?;
    let (body,old):(String,Option<String>)=db.query_row("SELECT encrypted_body,encrypted_receipt FROM daemon_enterprise_offline_batches WHERE request_id=?1 AND run_id=?2",params![id.to_string(),run.to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let b: Batch = open(daemon, &body)?;
    if receipt["format"] != 1
        || receipt["permit_id"] != p["permit_id"]
        || receipt["run_id"] != json!(run)
        || receipt["request_sha256"] != json!(digest(&b)?)
        || receipt["finished"] != json!(b.finished)
        || receipt["original_result"] != json!(b.result)
        || receipt["retained_as_original_fact"] != true
        || receipt["run_replayed"] != false
        || receipt["execution_stopped_claimed"] != false
    {
        bail!("actual reconnect receipt differs from original native batch")
    };
    if let Some(old) = old {
        if open::<Value>(daemon, &old)? != *receipt {
            bail!("different original reconnect receipt")
        }
    }
    db.execute("UPDATE daemon_enterprise_offline_batches SET encrypted_receipt=?1,acked=1 WHERE request_id=?2",params![seal(daemon,receipt)?,id.to_string()])?;
    Ok(receipt.clone())
}
// The bridge records this only after its actual result snapshot publication. Recovery
// may repeat these original bytes, never synthesize a completion from local state.
pub(super) async fn completion(daemon: &Daemon, params: Value) -> Result<Value> {
    let run = run_id_param(&params)?;
    let original = params["body"].clone();
    let body: cowork_contracts::CompleteRunRequest = serde_json::from_value(original.clone())?;
    let db = daemon.database.lock().await;
    let mut j = journal(daemon, &db, run)?.context("original offline journal missing")?;
    let local: RunRecord = serde_json::from_str(&db.query_row::<String, _, _>(
        "SELECT record_json FROM daemon_runs WHERE id=?1",
        [run.to_string()],
        |r| r.get(0),
    )?)?;
    if j.permit.is_none()
        || body.lease_token != j.lease.lease_token
        || digest(&local.spec)? != digest(&j.lease.run.spec)?
        || local.state != RunState::Completed
        || body.result != local.result.clone().unwrap_or(Value::Null)
        || serde_json::to_value(&body)? != original
    {
        bail!("actual bridge completion differs from the original lease and local result")
    }
    if let Some(old) = &j.completion {
        if *old != original {
            bail!("different immutable original completion body")
        }
    } else {
        j.completion = Some(original.clone());
        db.execute(
            "UPDATE daemon_enterprise_offline SET encrypted_original=?1 WHERE run_id=?2",
            params![seal(daemon, &j)?, run.to_string()],
        )?;
    }
    Ok(original)
}
pub(super) async fn settled(daemon: &Daemon, params: Value) -> Result<Value> {
    let run = run_id_param(&params)?;
    let receipt = params["receipt"].clone();
    let record: RunRecord = serde_json::from_value(receipt.clone())?;
    let db = daemon.database.lock().await;
    let mut j = journal(daemon, &db, run)?.context("original journal missing")?;
    let completion: cowork_contracts::CompleteRunRequest = serde_json::from_value(
        j.completion
            .clone()
            .context("original bridge completion missing")?,
    )?;
    let local: RunRecord = serde_json::from_str(&db.query_row::<String, _, _>(
        "SELECT record_json FROM daemon_runs WHERE id=?1",
        [run.to_string()],
        |r| r.get(0),
    )?)?;
    if digest(&record.spec)? != digest(&j.lease.run.spec)?
        || record.state != RunState::Completed
        || local.state != RunState::Completed
        || record.result != local.result
        || completion.result != record.result.clone().unwrap_or(Value::Null)
    {
        bail!("actual original completion receipt differs from the local run")
    };
    if let Some(old) = &j.settlement {
        if *old != receipt {
            bail!("different immutable completion receipt")
        }
    } else {
        j.settlement = Some(receipt.clone());
        db.execute(
            "UPDATE daemon_enterprise_offline SET encrypted_original=?1 WHERE run_id=?2",
            params![seal(daemon, &j)?, run.to_string()],
        )?;
    }
    Ok(receipt)
}
