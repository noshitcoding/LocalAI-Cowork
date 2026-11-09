//! Frozen, authenticated prefix exports and independently signed checkpoints.
use super::*;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepare {
    request_id: Uuid,
    expected_revision: i64,
    max_records: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Complete {
    export_sha256: String,
    witness_receipt: Value,
    confirmation: String,
}
fn conflict() -> ApiError {
    ApiError::Conflict("audit checkpoint, original export or witness binding changed".into())
}
pub(super) async fn lock(tx: &mut Transaction<'_, Postgres>, org: Uuid) -> Result<Value, ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("management-audit:{org}"))
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO management_audit_checkpoints(organisation_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(org).execute(&mut **tx).await?;
    let checkpoint: Value = sqlx::query_scalar("SELECT to_jsonb(c) FROM management_audit_checkpoints c WHERE organisation_id=$1 FOR UPDATE")
        .bind(org).fetch_one(&mut **tx).await?;
    verify_checkpoint(&checkpoint, org)?;
    Ok(checkpoint)
}
async fn reader(state: &AppState, principal: &Principal, org: Uuid) -> Result<(), ApiError> {
    if !matches!(
        membership(state, principal, org, false).await?.as_str(),
        "admin" | "auditor"
    ) {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn checkpoint(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    reader(&state, &principal, org).await?;
    let mut tx = state.pool.begin().await?;
    let value = lock(&mut tx, org).await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn prepare(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<Prepare>,
) -> Result<Json<Value>, ApiError> {
    reader(&state, &principal, org).await?;
    if request.request_id.is_nil()
        || request.expected_revision < 0
        || !(1..=10000).contains(&request.max_records)
    {
        return Err(invalid(
            "audit export requires an original request, checkpoint revision and 1..10000 records",
        ));
    }
    let request_sha = hex::encode(Sha256::digest(serde_json::to_vec(&request)?));
    let mut tx = state.pool.begin().await?;
    let base = lock(&mut tx, org).await?;
    let prior = sqlx::query("SELECT request_sha256,document,export_sha256 FROM management_audit_exports WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3")
        .bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?;
    if let Some(row) = prior {
        if row.get::<String, _>("request_sha256") != request_sha {
            return Err(conflict());
        }
        if row.get::<Value, _>("document")["archived"] == true {
            return Err(ApiError::Conflict(
                "the original export body expired; inspect its receipt or prepare a new request"
                    .into(),
            ));
        }
        return Ok(Json(
            json!({"document":row.get::<Value,_>("document"),"export_sha256":row.get::<String,_>("export_sha256")}),
        ));
    }
    if base["revision"].as_i64() != Some(request.expected_revision) {
        return Err(conflict());
    }
    let policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1 FOR SHARE")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
    let days = policy["audit_retention_days"]
        .as_i64()
        .ok_or_else(conflict)?;
    if !(1..=3650).contains(&days) {
        return Err(conflict());
    }
    let cutoff: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT clock_timestamp()-make_interval(days => $1)")
            .bind(days as i32)
            .fetch_one(&mut *tx)
            .await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM management_audit a WHERE organisation_id=$1 ORDER BY sequence LIMIT $2")
        .bind(org).bind(request.max_records).fetch_all(&mut *tx).await?;
    let key = audit_key().map_err(ApiError::Internal)?;
    let mut previous = base["digest"].as_str().ok_or_else(conflict)?.to_owned();
    let mut sequence = base["sequence"].as_i64().ok_or_else(conflict)?;
    let mut verified = Vec::new();
    let mut prune_sequence = sequence;
    let mut eligible = true;
    for row in rows {
        if row["previous_digest"].as_str() != Some(&previous)
            || row["sequence"].as_i64().is_none_or(|s| s <= sequence)
        {
            return Err(conflict());
        }
        let record = verified_audit_record(row, org, &key)?;
        let created = chrono::DateTime::parse_from_rfc3339(
            record["created_at"].as_str().ok_or_else(conflict)?,
        )
        .map_err(|_| conflict())?
        .with_timezone(&chrono::Utc);
        sequence = record["sequence"].as_i64().ok_or_else(conflict)?;
        previous = record["digest"].as_str().ok_or_else(conflict)?.to_owned();
        eligible &= created < cutoff;
        if eligible {
            prune_sequence = sequence;
        }
        verified.push(record);
    }
    let id = Uuid::new_v4();
    let document = json!({"format":"cowork-management-audit-export-v1","id":id,"organisation_id":org,"base":base,"records":verified,"end_sequence":sequence,"end_digest":previous,"prune_through":prune_sequence,"retention_days":days,"cutoff":cutoff});
    let export_sha = hex::encode(Sha256::digest(serde_json::to_vec(&document)?));
    sqlx::query("INSERT INTO management_audit_exports(id,organisation_id,actor_user_id,request_id,request_sha256,base_revision,export_sha256,document,retain_until) VALUES($1,$2,$3,$4,$5,$6,$7,$8,clock_timestamp()+make_interval(days => $9))")
        .bind(id).bind(org).bind(principal.user_id).bind(request.request_id).bind(request_sha).bind(request.expected_revision).bind(&export_sha).bind(&document).bind(days as i32).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"document":document,"export_sha256":export_sha}),
    ))
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    reader(&state, &principal, org).await?;
    let value = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM management_audit_exports e WHERE organisation_id=$1 AND id=$2",
    )
    .bind(org)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(denied)?;
    Ok(Json(value))
}
fn signature(receipt: &Value) -> Result<(), ApiError> {
    let payload = &receipt["payload"];
    let expected_id = std::env::var("COWORK_AUDIT_WITNESS_ID")
        .map_err(|_| ApiError::Unavailable("external audit witness is not configured".into()))?;
    if payload["format"] != "cowork-audit-witness-v1" || payload["witness_id"] != expected_id {
        return Err(conflict());
    }
    let path = std::env::var("COWORK_AUDIT_WITNESS_PUBLIC_KEY_FILE").map_err(|_| {
        ApiError::Unavailable("external audit witness public key is not configured".into())
    })?;
    let key =
        PKey::public_key_from_pem(&std::fs::read(path).map_err(|e| ApiError::Internal(e.into()))?)
            .map_err(|_| conflict())?;
    let signature = hex::decode(receipt["signature_hex"].as_str().ok_or_else(conflict)?)
        .map_err(|_| conflict())?;
    let mut verify = Verifier::new(MessageDigest::sha256(), &key).map_err(|_| conflict())?;
    verify
        .update(&serde_json::to_vec(payload)?)
        .map_err(|_| conflict())?;
    if !verify.verify(&signature).map_err(|_| conflict())? {
        return Err(conflict());
    }
    Ok(())
}
pub(super) fn verify_checkpoint(checkpoint: &Value, org: Uuid) -> Result<(), ApiError> {
    if checkpoint["revision"] == 0 {
        if checkpoint["sequence"] != 0
            || checkpoint["digest"] != "0".repeat(64)
            || !checkpoint["witness_receipt"].is_null()
        {
            return Err(conflict());
        }
    } else {
        let receipt = &checkpoint["witness_receipt"];
        signature(receipt)?;
        if receipt["payload"]["organisation_id"] != json!(org)
            || receipt["payload"]["checkpoint_digest"] != checkpoint["digest"]
            || receipt["payload"]["checkpoint_sequence"] != checkpoint["sequence"]
        {
            return Err(conflict());
        }
    }
    Ok(())
}
fn witness(receipt: &Value, document: &Value, export_sha: &str) -> Result<(), ApiError> {
    signature(receipt)?;
    let payload = &receipt["payload"];
    let through = document["prune_through"].as_i64().ok_or_else(conflict)?;
    let checkpoint_digest = document["records"]
        .as_array()
        .ok_or_else(conflict)?
        .iter()
        .find(|r| r["sequence"].as_i64() == Some(through))
        .map(|r| &r["digest"])
        .unwrap_or(&document["base"]["digest"]);
    if payload["export_id"] != document["id"]
        || payload["organisation_id"] != document["organisation_id"]
        || payload["export_sha256"] != export_sha
        || payload["base_digest"] != document["base"]["digest"]
        || payload["end_digest"] != document["end_digest"]
        || payload["end_sequence"] != document["end_sequence"]
        || payload["checkpoint_sequence"] != document["prune_through"]
        || &payload["checkpoint_digest"] != checkpoint_digest
    {
        return Err(conflict());
    }
    Ok(())
}
pub(super) async fn complete(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(request): Json<Complete>,
) -> Result<Json<Value>, ApiError> {
    if request.confirmation != "ANCHOR_AND_APPLY_AUDIT_RETENTION" {
        return Err(invalid("confirm the original export and retention plan"));
    }
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let base = lock(&mut tx, org).await?;
    let row = sqlx::query("SELECT document,export_sha256,witness_receipt,final_receipt FROM management_audit_exports WHERE organisation_id=$1 AND id=$2 FOR UPDATE")
        .bind(org).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if request.export_sha256 != row.get::<String, _>("export_sha256") {
        return Err(conflict());
    }
    if let Some(receipt) = row.get::<Option<Value>, _>("final_receipt") {
        if row.get::<Option<Value>, _>("witness_receipt").as_ref() != Some(&request.witness_receipt)
        {
            return Err(conflict());
        }
        return Ok(Json(receipt));
    }
    let document: Value = row.get("document");
    if hex::encode(Sha256::digest(serde_json::to_vec(&document)?)) != request.export_sha256 {
        return Err(conflict());
    }
    witness(&request.witness_receipt, &document, &request.export_sha256)?;
    if base != document["base"] {
        return Err(conflict());
    }
    // Recheck every original row under the writer lock immediately before deletion.
    let current: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM management_audit a WHERE organisation_id=$1 AND sequence <= $2 ORDER BY sequence")
        .bind(org).bind(document["end_sequence"].as_i64().ok_or_else(conflict)?).fetch_all(&mut *tx).await?;
    let key = audit_key().map_err(ApiError::Internal)?;
    let authenticated: Vec<Value> = current
        .into_iter()
        .map(|r| verified_audit_record(r, org, &key))
        .collect::<Result<_, _>>()?;
    if json!(authenticated) != document["records"] {
        return Err(conflict());
    }
    // A shortened retention policy cannot silently change an old plan; extending it
    // must not delete records under the superseded shorter policy either.
    let policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1 FOR SHARE")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
    if policy["audit_retention_days"] != document["retention_days"] {
        return Err(conflict());
    }
    let through = document["prune_through"].as_i64().ok_or_else(conflict)?;
    let digest = authenticated
        .iter()
        .find(|r| r["sequence"].as_i64() == Some(through))
        .map(|r| r["digest"].clone())
        .unwrap_or_else(|| base["digest"].clone());
    // Always retain this authenticated event, so ordinary append never starts a
    // fresh genesis even if the exported prefix contained the entire old chain.
    append_audit(&mut tx,org,principal.user_id,"audit.retention",id,json!({"export_sha256":request.export_sha256,"through":through,"witness_receipt":request.witness_receipt})).await?;
    let removed =
        sqlx::query("DELETE FROM management_audit WHERE organisation_id=$1 AND sequence <= $2")
            .bind(org)
            .bind(through)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    let revision = base["revision"].as_i64().ok_or_else(conflict)? + 1;
    let receipt = json!({"format":"cowork-audit-retention-receipt-v1","export_id":id,"organisation_id":org,"export_sha256":request.export_sha256,"revision":revision,"removed_records":removed,"checkpoint_sequence":through,"checkpoint_digest":digest,"witness_receipt":request.witness_receipt});
    sqlx::query("UPDATE management_audit_checkpoints SET revision=$2,sequence=$3,digest=$4,witness_receipt=$5 WHERE organisation_id=$1")
        .bind(org).bind(revision).bind(through).bind(digest.as_str().ok_or_else(conflict)?).bind(&request.witness_receipt).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE management_audit_exports SET witness_receipt=$2,final_receipt=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(&request.witness_receipt)
    .bind(&receipt)
    .execute(&mut *tx)
    .await?;
    // Frozen exported bodies, including abandoned exports, have their own policy
    // window. Checkpoints and original final receipts survive compaction.
    sqlx::query("UPDATE management_audit_exports SET document=jsonb_build_object('archived',true,'export_id',id,'export_sha256',export_sha256) WHERE organisation_id=$1 AND id<>$2 AND retain_until<clock_timestamp() AND document->>'archived' IS DISTINCT FROM 'true'")
        .bind(org).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
