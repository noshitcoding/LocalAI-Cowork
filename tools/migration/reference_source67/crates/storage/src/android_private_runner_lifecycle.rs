//! Original create dispatch and proved no-send closure. No physical evidence is inferred here.
use super::*;
use sqlx::any::AnyRow;

/// A single original dispatcher owns this value until it consumes it for IO.
/// It cannot be cloned, deserialized or reconstructed from public receipt bytes.
pub struct PrivateCreateDispatch {
    nonce: Uuid,
    binding: Value,
    original: Value,
    receipt: Value,
}
impl PrivateCreateDispatch {
    pub fn original_receipt(&self) -> &Value {
        &self.receipt
    }
    pub fn begin_io(self) {}
}
pub(super) async fn original(tx: &mut Transaction<'_, Any>, c: &AnyRow) -> Result<Value> {
    let run = id(c, "run_id")?;
    let lease = id(c, "lease_id")?;
    let invocation: String = c.try_get("invocation_id").map_err(storage_error)?;
    let binding: Value = parse(
        &c.try_get::<String, _>("original_binding")
            .map_err(storage_error)?,
    )?;
    checked_binding(&binding)?;
    let request: String = if let Some(raw) = invocation.strip_prefix("terminal:") {
        let sid = Uuid::parse_str(raw).map_err(storage_error)?;
        sqlx::query_scalar("SELECT request_sha FROM android_private_terminal_sessions WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND container_nonce=$6").bind(sid.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(id(c,"owner_id")?.to_string()).bind(id(c,"project_id")?.to_string()).bind(id(c,"id")?.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?
    } else {
        let original = invocation
            .strip_prefix("stdio:")
            .map(|v| format!("mcp-discovery:{v}"))
            .unwrap_or_else(|| invocation.clone());
        sqlx::query_scalar("SELECT request_sha FROM android_private_loop_steps WHERE run_id=$1 AND lease_id=$2 AND invocation_id=$3").bind(run.to_string()).bind(lease.to_string()).bind(original).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?
    };
    let expected = canonical_hash(
        &json!({"format":1,"owner":id(c,"owner_id")?,"project":id(c,"project_id")?,"run":run,"nonce":id(c,"id")?,"invocation":invocation,"binding":binding,"original_request_sha256":request}),
    );
    if c.try_get::<String, _>("creation_sha")
        .map_err(storage_error)?
        != expected
    {
        return Err(Error::Conflict);
    }
    Ok(
        json!({"format":"private-container-original/1","nonce":id(c,"id")?,"run_id":run,"lease_id":lease,"owner_id":id(c,"owner_id")?,"project_id":id(c,"project_id")?,"invocation_id":invocation,"binding":binding,"creation_sha256":expected,"original_request_sha256":request,"created_at":c.try_get::<i64,_>("created_at").map_err(storage_error)?}),
    )
}
async fn lock_record(
    tx: &mut Transaction<'_, Any>,
    nonce: Uuid,
    binding: &Value,
) -> Result<AnyRow> {
    sqlx::query("UPDATE android_private_containers SET revision=revision WHERE id=$1 AND original_binding=$2").bind(nonce.to_string()).bind(json_string(binding)?).execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("SELECT * FROM android_private_containers WHERE id=$1 AND original_binding=$2")
        .bind(nonce.to_string())
        .bind(json_string(binding)?)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Forbidden)
}
fn receipt_hash(value: &Value, key: &str) -> Result<String> {
    let mut unsigned = value.clone();
    let recorded = unsigned
        .get_mut(key)
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(Error::Conflict)?;
    unsigned[key] = json!("");
    if canonical_hash(&unsigned) != recorded {
        return Err(Error::Conflict);
    }
    Ok(recorded)
}
fn dispatch_receipt(raw: &str, original: &Value) -> Result<Value> {
    let v: Value = parse(raw)?;
    if v["format"] != "private-container-create-dispatch/1"
        || v["original"] != *original
        || v["send_count"] != 1
        || v["admitted_at"].as_i64().is_none()
    {
        return Err(Error::Conflict);
    }
    receipt_hash(&v, "receipt_sha256")?;
    Ok(v)
}
pub(super) fn check_unsent(c: &AnyRow, original: &Value) -> Result<Option<Value>> {
    let Some(raw) = c
        .try_get::<Option<String>, _>("original_unsent_receipt")
        .map_err(storage_error)?
    else {
        return Ok(None);
    };
    let v: Value = parse(&raw)?;
    let admitted = c
        .try_get::<i64, _>("create_dispatch_admitted")
        .map_err(storage_error)?;
    if c.try_get::<String, _>("state").map_err(storage_error)? != "prepared"
        || c.try_get::<Option<String>, _>("container_id")
            .map_err(storage_error)?
            .is_some()
        || c.try_get::<Option<String>, _>("creation_receipt")
            .map_err(storage_error)?
            .is_some()
        || c.try_get::<Option<String>, _>("termination_witness")
            .map_err(storage_error)?
            .is_some()
        || v["format"] != "private-container-create-unsent/1"
        || v["original"] != *original
        || v["never_sent"] != true
        || v["physical_evidence"] != false
        || v["closed_at"].as_i64().is_none()
    {
        return Err(Error::Conflict);
    }
    match (
        admitted,
        c.try_get::<Option<String>, _>("original_dispatch_receipt")
            .map_err(storage_error)?,
    ) {
        (0, None)
            if v["proof"] == "never_dispatch_admitted"
                && v["original_dispatch_sha256"].is_null() => {}
        (1, Some(raw)) if v["proof"] == "owned_unconsumed_dispatch_permit" => {
            let dispatch = dispatch_receipt(&raw, original)?;
            if v["original_dispatch_sha256"] != dispatch["receipt_sha256"] {
                return Err(Error::Conflict);
            }
        }
        _ => return Err(Error::Conflict),
    }
    receipt_hash(&v, "receipt_sha256")?;
    if let Some(raw) = original["invocation_id"]
        .as_str()
        .and_then(|s| s.strip_prefix("terminal:"))
    {
        let sid = Uuid::parse_str(raw).map_err(storage_error)?;
        let expected = json!({"format":"private-terminal-no-start/1","session_id":sid,"run_id":original["run_id"],"lease_id":original["lease_id"],"owner_id":original["owner_id"],"project_id":original["project_id"],"container_nonce":original["nonce"],"create_sent":false,"guest_started":false,"input_sequence":0,"output_sequence":0,"physical_evidence":false,"files_promoted":false,"execution_adopted":false});
        if v["terminal_no_start"] != expected {
            return Err(Error::Conflict);
        }
    } else if !v["terminal_no_start"].is_null() {
        return Err(Error::Conflict);
    }
    Ok(Some(v))
}
pub(super) fn record_value(c: &AnyRow, now: i64) -> Result<Value> {
    let current = c
        .try_get::<Option<String>, _>("current_lease")
        .map_err(storage_error)?;
    let own = c.try_get::<String, _>("lease_id").map_err(storage_error)?;
    let active = current.as_deref() == Some(own.as_str())
        && c.try_get::<i64, _>("lease_until").map_err(storage_error)? > now
        && matches!(
            c.try_get::<String, _>("run_state")
                .map_err(storage_error)?
                .as_str(),
            "running" | "waiting_for_approval" | "waiting_for_input"
        );
    Ok(
        json!({"id":id(c,"id")?,"run_id":id(c,"run_id")?,"owner_id":id(c,"owner_id")?,"project_id":id(c,"project_id")?,"binding":parse::<Value>(&c.try_get::<String,_>("original_binding").map_err(storage_error)?)?,"container_id":c.try_get::<Option<String>,_>("container_id").map_err(storage_error)?,"state":c.try_get::<String,_>("state").map_err(storage_error)?,"creation_sha256":c.try_get::<String,_>("creation_sha").map_err(storage_error)?,"revision":c.try_get::<i64,_>("revision").map_err(storage_error)?,"lease_active":active,"create_dispatch_admitted":c.try_get::<i64,_>("create_dispatch_admitted").map_err(storage_error)?,"original_dispatch_receipt":c.try_get::<Option<String>,_>("original_dispatch_receipt").map_err(storage_error)?.map(|v|parse::<Value>(&v)).transpose()?,"original_unsent_receipt":c.try_get::<Option<String>,_>("original_unsent_receipt").map_err(storage_error)?.map(|v|parse::<Value>(&v)).transpose()?,"termination_witness":c.try_get::<Option<String>,_>("termination_witness").map_err(storage_error)?.map(|v|parse::<Value>(&v)).transpose()?}),
    )
}
impl Store {
    pub async fn android_private_container_record(
        &self,
        nonce: Uuid,
        binding: &Value,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let c=sqlx::query("SELECT c.*,r.state AS run_state,r.lease_id AS current_lease,r.lease_until FROM android_private_containers c JOIN android_private_server_runs r ON r.id=c.run_id WHERE c.id=$1 AND c.original_binding=$2").bind(nonce.to_string()).bind(json_string(binding)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let value = record_value(&c, now)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// Positive original no-create proof for retirement consumers; never a stop
    /// or file-promotion permit. The caller already owns its transaction locks.
    pub(crate) async fn android_private_container_no_create_receipt(
        &self,
        tx: &mut Transaction<'_, Any>,
        nonce: Uuid,
    ) -> Result<Option<Value>> {
        let c = sqlx::query("SELECT * FROM android_private_containers WHERE id=$1")
            .bind(nonce.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let original = original(tx, &c).await?;
        check_unsent(&c, &original)
    }
    pub async fn android_private_container_create_dispatch<F: Fn() -> bool + Send + Sync>(
        &self,
        run: Uuid,
        lease: Uuid,
        nonce: Uuid,
        binding: &Value,
        keys: &dyn KeyStore,
        can_send: F,
    ) -> Result<PrivateCreateDispatch> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let now = device_writer(&mut tx).await?;
        leased(&mut tx, run, lease, now).await?;
        let c = lock_record(&mut tx, nonce, binding).await?;
        let original = original(&mut tx, &c).await?;
        // Same transaction contains the last crypto/ACL/runner checks after
        // acquiring the original row and before its single-use dispatch CAS.
        self.private_create_validate_in(&mut tx, run, lease, nonce, binding, keys)
            .await?;
        if id(&c, "run_id")? != run || id(&c, "lease_id")? != lease || !can_send() {
            return Err(Error::Forbidden);
        }
        let admitted_at = device_writer(&mut tx).await?;
        leased(&mut tx, run, lease, admitted_at).await?;
        let mut receipt = json!({"format":"private-container-create-dispatch/1","original":original,"send_count":1,"admitted_at":admitted_at,"receipt_sha256":""});
        receipt["receipt_sha256"] = json!(canonical_hash(&receipt));
        let n=sqlx::query("UPDATE android_private_containers SET create_dispatch_admitted=1,original_dispatch_receipt=$1,revision=revision+1 WHERE id=$2 AND run_id=$3 AND lease_id=$4 AND original_binding=$5 AND creation_sha=$6 AND state='prepared' AND container_id IS NULL AND creation_receipt IS NULL AND termination_witness IS NULL AND create_dispatch_admitted=0 AND original_dispatch_receipt IS NULL AND original_unsent_receipt IS NULL AND revision=$7").bind(json_string(&receipt)?).bind(nonce.to_string()).bind(run.to_string()).bind(lease.to_string()).bind(json_string(binding)?).bind(c.try_get::<String,_>("creation_sha").map_err(storage_error)?).bind(c.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if n != 1 {
            return Err(Error::UncertainEffect);
        }
        if !can_send() {
            return Err(Error::Forbidden);
        }
        let permit = PrivateCreateDispatch {
            nonce,
            binding: binding.clone(),
            original,
            receipt,
        };
        if let Err(error) = tx.commit().await {
            // This original dispatcher has not invoked IO. Preserve that fact
            // for either side of an uncertain SQL commit, rather than resend.
            self.android_private_container_close_unconsumed(permit)
                .await?;
            return Err(storage_error(error));
        }
        Ok(permit)
    }
    pub async fn android_private_container_close_unadmitted(
        &self,
        nonce: Uuid,
        binding: &Value,
    ) -> Result<Value> {
        self.private_close_create_unsent(nonce, binding, None).await
    }
    pub async fn android_private_container_close_unconsumed(
        &self,
        permit: PrivateCreateDispatch,
    ) -> Result<Value> {
        self.private_close_create_unsent(
            permit.nonce,
            &permit.binding,
            Some((&permit.original, &permit.receipt)),
        )
        .await
    }
    async fn private_close_create_unsent(
        &self,
        nonce: Uuid,
        binding: &Value,
        permit: Option<(&Value, &Value)>,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let c = lock_record(&mut tx, nonce, binding).await?;
        let original = original(&mut tx, &c).await?;
        if let Some(retained) = check_unsent(&c, &original)? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(retained);
        }
        let admitted = c
            .try_get::<i64, _>("create_dispatch_admitted")
            .map_err(storage_error)?;
        let dispatch = c
            .try_get::<Option<String>, _>("original_dispatch_receipt")
            .map_err(storage_error)?;
        let (proof, dispatch_sha) = match (admitted, dispatch.as_deref(), permit) {
            (0, None, None) => ("never_dispatch_admitted", Value::Null),
            (0, None, Some((expected, _))) if expected == &original => {
                ("never_dispatch_admitted", Value::Null)
            }
            (1, Some(raw), Some((expected, owned))) if expected == &original => {
                let recorded = dispatch_receipt(raw, &original)?;
                if &recorded != owned {
                    return Err(Error::Conflict);
                }
                (
                    "owned_unconsumed_dispatch_permit",
                    recorded["receipt_sha256"].clone(),
                )
            }
            _ => return Err(Error::UncertainEffect),
        };
        if c.try_get::<String, _>("state").map_err(storage_error)? != "prepared"
            || c.try_get::<Option<String>, _>("container_id")
                .map_err(storage_error)?
                .is_some()
            || c.try_get::<Option<String>, _>("creation_receipt")
                .map_err(storage_error)?
                .is_some()
            || c.try_get::<Option<String>, _>("termination_witness")
                .map_err(storage_error)?
                .is_some()
        {
            return Err(Error::Conflict);
        }
        let terminal = if let Some(raw) = original["invocation_id"]
            .as_str()
            .and_then(|s| s.strip_prefix("terminal:"))
        {
            let sid = Uuid::parse_str(raw).map_err(storage_error)?;
            let s=sqlx::query("SELECT revision,input_sequence,output_sequence,input_bytes,output_bytes FROM android_private_terminal_sessions WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND container_nonce=$6 AND state='connected' AND receipt_version IS NULL AND publication_version IS NULL").bind(sid.to_string()).bind(id(&c,"run_id")?.to_string()).bind(id(&c,"lease_id")?.to_string()).bind(id(&c,"owner_id")?.to_string()).bind(id(&c,"project_id")?.to_string()).bind(nonce.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            for k in [
                "input_sequence",
                "output_sequence",
                "input_bytes",
                "output_bytes",
            ] {
                if s.try_get::<i64, _>(k).map_err(storage_error)? != 0 {
                    return Err(Error::Conflict);
                }
            }
            let io:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM android_private_terminal_inputs WHERE session_id=$1)+(SELECT COUNT(*) FROM android_private_terminal_outputs WHERE session_id=$1)").bind(sid.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if io != 0 {
                return Err(Error::Conflict);
            }
            if sqlx::query("UPDATE android_private_terminal_sessions SET state='closed',revision=revision+1 WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND container_nonce=$6 AND state='connected' AND revision=$7 AND receipt_version IS NULL AND publication_version IS NULL AND input_sequence=0 AND output_sequence=0 AND input_bytes=0 AND output_bytes=0").bind(sid.to_string()).bind(id(&c,"run_id")?.to_string()).bind(id(&c,"lease_id")?.to_string()).bind(id(&c,"owner_id")?.to_string()).bind(id(&c,"project_id")?.to_string()).bind(nonce.to_string()).bind(s.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
            json!({"format":"private-terminal-no-start/1","session_id":sid,"run_id":id(&c,"run_id")?,"lease_id":id(&c,"lease_id")?,"owner_id":id(&c,"owner_id")?,"project_id":id(&c,"project_id")?,"container_nonce":nonce,"create_sent":false,"guest_started":false,"input_sequence":0,"output_sequence":0,"physical_evidence":false,"files_promoted":false,"execution_adopted":false})
        } else {
            Value::Null
        };
        let mut receipt = json!({"format":"private-container-create-unsent/1","original":original,"proof":proof,"original_dispatch_sha256":dispatch_sha,"never_sent":true,"physical_evidence":false,"terminal_no_start":terminal,"closed_at":now,"receipt_sha256":""});
        receipt["receipt_sha256"] = json!(canonical_hash(&receipt));
        let n=sqlx::query("UPDATE android_private_containers SET original_unsent_receipt=$1,revision=revision+1 WHERE id=$2 AND original_binding=$3 AND creation_sha=$4 AND create_dispatch_admitted=$5 AND state='prepared' AND container_id IS NULL AND creation_receipt IS NULL AND termination_witness IS NULL AND original_unsent_receipt IS NULL AND revision=$6").bind(json_string(&receipt)?).bind(nonce.to_string()).bind(json_string(binding)?).bind(c.try_get::<String,_>("creation_sha").map_err(storage_error)?).bind(admitted).bind(c.try_get::<i64,_>("revision").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if n != 1 {
            return Err(Error::Conflict);
        }
        crate::android_private_server::event(&mut tx,id(&c,"run_id")?,if terminal.is_null(){"private_container_create_unsent"}else{"private_terminal_never_started"},&json!({"container_nonce":nonce,"original_creation_sha256":receipt["original"]["creation_sha256"],"original_unsent_receipt_sha256":receipt["receipt_sha256"],"terminal_no_start":terminal,"physical_evidence":false,"execution_adopted":false}),now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
