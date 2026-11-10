//! Central managed-model budgets. A durable reservation precedes every bounded
//! provider request; uncertainty keeps the full reservation blocked. The ledger
//! records the explicit internal tariff, not an inferred provider invoice.
use super::*;
use serde::Serialize;

pub(crate) const MAX_MICROS: i64 = 9_000_000_000_000;

#[derive(Debug)]
pub(crate) struct WalletReservationRequest {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
    pub model_resource_id: Uuid,
    pub request_id: Uuid,
    pub request_sha256: String,
    pub maximum_micros: Option<i64>,
    pub currency: Option<String>,
    pub metadata: Value,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct WalletReservation {
    pub id: Uuid,
    pub state: String,
    pub maximum_micros: i64,
}
#[derive(Debug)]
pub(crate) struct WalletReservationDecision {
    pub enforced: bool,
    pub reservation: Option<WalletReservation>,
    pub replayed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Update {
    expected_revision: i64,
    enabled: bool,
    monthly_limit_micros: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    request_id: Uuid,
    expected_revision: i64,
    kind: String,
    amount_micros: i64,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsUpdate {
    expected_revision: i64,
    currency: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolve {
    request_id: Uuid,
    expected_revision: i64,
    actual_micros: i64,
    reason: String,
}

fn micros(value: i64) -> Result<(), ApiError> {
    if !(0..=MAX_MICROS).contains(&value) {
        return Err(invalid(
            "money must be a nonnegative integer of at most 9000000000000 microcurrency units",
        ));
    }
    Ok(())
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn hash(value: &Value) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(value).expect("JSON value"),
    ))
}
fn metadata_valid(value: &Value) -> bool {
    value.is_object() && value.to_string().len() <= 8192
}
fn conflict() -> ApiError {
    ApiError::Conflict(
        "reload the wallet before saving; its balance, reservations or settings changed".into(),
    )
}

async fn administrator(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
) -> Result<(), ApiError> {
    sso::authorize(tx, org, principal).await?;
    job_authority::capture(state, tx, principal, org).await?;
    Ok(())
}
async fn readable(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
    user: Option<Uuid>,
) -> Result<bool, ApiError> {
    organisation_lifecycle::share_active(tx, org).await?;
    if principal.bootstrap {
        return Err(denied());
    }
    let row = sqlx::query("SELECT u.platform_admin,m.role FROM users u LEFT JOIN management_members m ON m.organisation_id=$1 AND m.user_id=u.id WHERE u.id=$2 AND u.deleted_at IS NULL FOR SHARE OF u")
        .bind(org).bind(principal.user_id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let admin = row.get::<bool, _>("platform_admin")
        || row.get::<Option<String>, _>("role").as_deref() == Some("admin");
    if !admin
        && (row.get::<Option<String>, _>("role").is_none()
            || user.is_some_and(|id| id != principal.user_id))
    {
        return Err(denied());
    }
    job_authority::capture(state, tx, principal, org).await?;
    Ok(admin)
}
async fn target(tx: &mut Transaction<'_, Postgres>, org: Uuid, user: Uuid) -> Result<(), ApiError> {
    sqlx::query("SELECT m.user_id FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.user_id=$2 AND u.deleted_at IS NULL FOR SHARE OF m,u")
        .bind(org).bind(user).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    Ok(())
}
async fn lock_wallet(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    sqlx::query(
        "SELECT organisation_id FROM management_wallet_settings WHERE organisation_id=$1 FOR SHARE",
    )
    .bind(org)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO management_member_wallets(organisation_id,user_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(org).bind(user).execute(&mut **tx).await?;
    Ok(sqlx::query("SELECT w.*,s.currency FROM management_member_wallets w JOIN management_wallet_settings s ON s.organisation_id=w.organisation_id WHERE w.organisation_id=$1 AND w.user_id=$2 FOR UPDATE OF w")
        .bind(org).bind(user).fetch_one(&mut **tx).await?)
}
async fn totals(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
) -> Result<(i64, i64, i64), ApiError> {
    let row = sqlx::query("SELECT COALESCE(sum(maximum_micros) FILTER(WHERE state IN ('reserved','held')),0)::BIGINT reserved,COALESCE(sum(maximum_micros) FILTER(WHERE state IN ('reserved','held') AND period_start=(date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC'))::DATE),0)::BIGINT monthly_reserved,COALESCE(sum(charged_micros) FILTER(WHERE state='settled' AND period_start=(date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC'))::DATE),0)::BIGINT monthly_spent FROM management_wallet_reservations WHERE organisation_id=$1 AND user_id=$2")
        .bind(org).bind(user).fetch_one(&mut **tx).await?;
    Ok((
        row.get("reserved"),
        row.get("monthly_reserved"),
        row.get("monthly_spent"),
    ))
}
async fn current(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
    detail: bool,
) -> Result<Value, ApiError> {
    let row = lock_wallet(tx, org, user).await?;
    let identity = sqlx::query("SELECT email,display_name FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    let (reserved, monthly_reserved, monthly_spent) = totals(tx, org, user).await?;
    let enabled: bool = row.get("enabled");
    let period: String = sqlx::query_scalar(
        "SELECT (date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC'))::DATE::TEXT",
    )
    .fetch_one(&mut **tx)
    .await?;
    let mut value = json!({"organisation_id":org,"user_id":user,"email":identity.get::<String,_>("email"),"display_name":identity.get::<Option<String>,_>("display_name"),"currency":row.get::<String,_>("currency"),"revision":row.get::<i64,_>("revision"),"enabled":enabled,"balance_micros":row.get::<i64,_>("balance_micros"),"reserved_micros":reserved,"available_micros":row.get::<i64,_>("balance_micros")-reserved,"monthly_limit_micros":row.get::<Option<i64>,_>("monthly_limit_micros"),"monthly_spent_micros":monthly_spent,"monthly_reserved_micros":monthly_reserved,"period_start":period,"enforcement":{"state":if enabled{"enforced"}else{"disabled"},"coverage":"server_managed_models","automatic_charging":true}});
    if detail {
        let mut entries:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'request_id',request_id,'kind',kind,'amount_micros',amount_micros,'balance_after_micros',balance_after_micros,'reason',reason,'created_at',created_at,'actor_user_id',actor_user_id) FROM management_wallet_ledger WHERE organisation_id=$1 AND user_id=$2 ORDER BY created_at DESC,id DESC LIMIT 101")
            .bind(org).bind(user).fetch_all(&mut **tx).await?;
        let more = entries.len() > 100;
        entries.truncate(100);
        // A crashed/cancelled proxy can leave a durable reservation without its
        // final hold update. After twice its 300s HTTP timeout, expose the same
        // conservative review state; reading never releases or mutates funds.
        let reservations:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'state',CASE WHEN state='reserved' AND created_at<clock_timestamp()-interval '10 minutes' THEN 'held' ELSE state END,'maximum_micros',maximum_micros,'charged_micros',charged_micros,'created_at',created_at) FROM management_wallet_reservations WHERE organisation_id=$1 AND user_id=$2 ORDER BY (state<>'settled') DESC,created_at DESC LIMIT 100")
            .bind(org).bind(user).fetch_all(&mut **tx).await?;
        value["entries"] = json!(entries);
        value["reservations"] = json!(reservations);
        value["has_more"] = json!(more);
    }
    Ok(value)
}

pub(super) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let admin = readable(&state, &mut tx, &principal, org, None).await?;
    let users:Vec<Uuid>=sqlx::query_scalar("SELECT user_id FROM (SELECT m.user_id FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND u.deleted_at IS NULL AND ($3 OR m.user_id=$2) UNION SELECT w.user_id FROM management_member_wallets w WHERE w.organisation_id=$1 AND $3) visible ORDER BY user_id")
        .bind(org).bind(principal.user_id).bind(admin).fetch_all(&mut *tx).await?;
    let mut wallets = Vec::new();
    for user in users {
        wallets.push(current(&mut tx, org, user, false).await?);
    }
    let settings = sqlx::query(
        "SELECT currency,revision FROM management_wallet_settings WHERE organisation_id=$1",
    )
    .bind(org)
    .fetch_one(&mut *tx)
    .await?;
    let locked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_wallet_ledger WHERE organisation_id=$1) OR EXISTS(SELECT 1 FROM management_wallet_reservations WHERE organisation_id=$1)").bind(org).fetch_one(&mut *tx).await?;
    let result = json!({"organisation_id":org,"currency":settings.get::<String,_>("currency"),"settings_revision":settings.get::<i64,_>("revision"),"currency_locked":locked,"coverage":"managed_model_proxy","wallets":wallets});
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let admin = readable(&state, &mut tx, &principal, org, Some(user)).await?;
    let historical:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_member_wallets WHERE organisation_id=$1 AND user_id=$2)").bind(org).bind(user).fetch_one(&mut *tx).await?;
    if !admin || !historical {
        target(&mut tx, org, user).await?;
    }
    let result = current(&mut tx, org, user, true).await?;
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, user)): Path<(Uuid, Uuid)>,
    Json(request): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    if request.expected_revision < 1 {
        return Err(invalid("a current wallet revision is required"));
    }
    if let Some(limit) = request.monthly_limit_micros {
        micros(limit)?;
    }
    let mut tx = state.pool.begin().await?;
    administrator(&state, &mut tx, &principal, org).await?;
    target(&mut tx, org, user).await?;
    let row = lock_wallet(&mut tx, org, user).await?;
    if row.get::<i64, _>("revision") != request.expected_revision {
        return Err(conflict());
    }
    let (_, reserved, spent) = totals(&mut tx, org, user).await?;
    if request
        .monthly_limit_micros
        .is_some_and(|limit| limit < spent + reserved)
    {
        return Err(ApiError::Conflict(
            "the monthly limit cannot be lower than settled usage plus outstanding reservations"
                .into(),
        ));
    }
    sqlx::query("UPDATE management_member_wallets SET enabled=$3,monthly_limit_micros=$4,revision=revision+1,updated_by=$5,updated_at=clock_timestamp() WHERE organisation_id=$1 AND user_id=$2")
        .bind(org).bind(user).bind(request.enabled).bind(request.monthly_limit_micros).bind(principal.user_id).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "wallet.budget.changed",
        user,
        json!({"enabled":request.enabled,"monthly_limit_micros":request.monthly_limit_micros}),
    )
    .await?;
    let result = current(&mut tx, org, user, true).await?;
    tx.commit().await?;
    Ok(Json(result))
}

async fn post_ledger(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
    request_id: Uuid,
    kind: &str,
    amount: i64,
    reason: &str,
    actor: Option<Uuid>,
    reservation: Option<Uuid>,
    request_hash: &str,
) -> Result<(), ApiError> {
    let row = lock_wallet(tx, org, user).await?;
    let balance: i64 = row.get("balance_micros");
    let revision: i64 = row.get("revision");
    let after = if kind == "credit" {
        balance.checked_add(amount)
    } else {
        balance.checked_sub(amount)
    }
    .ok_or_else(|| invalid("wallet amount overflow"))?;
    micros(after)?;
    if kind == "debit" {
        let (reserved, _, _) = totals(tx, org, user).await?;
        if after < reserved {
            return Err(ApiError::Conflict(
                "reserved provider costs cannot be withdrawn".into(),
            ));
        }
    }
    sqlx::query("UPDATE management_member_wallets SET balance_micros=$3,revision=revision+1,updated_by=$4,updated_at=clock_timestamp() WHERE organisation_id=$1 AND user_id=$2")
        .bind(org).bind(user).bind(after).bind(actor).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO management_wallet_ledger(id,organisation_id,user_id,request_id,kind,amount_micros,balance_after_micros,currency,wallet_revision,reason,actor_user_id,reservation_id,request_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
        .bind(Uuid::new_v4()).bind(org).bind(user).bind(request_id).bind(kind).bind(amount).bind(after).bind(row.get::<String,_>("currency")).bind(revision+1).bind(reason).bind(actor).bind(reservation).bind(request_hash).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn entry(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, user)): Path<(Uuid, Uuid)>,
    Json(request): Json<Entry>,
) -> Result<Json<Value>, ApiError> {
    micros(request.amount_micros)?;
    let reason = request.reason.trim();
    if request.request_id.is_nil()
        || request.expected_revision < 1
        || request.amount_micros == 0
        || !matches!(request.kind.as_str(), "credit" | "debit")
        || reason.is_empty()
        || reason.chars().count() > 500
    {
        return Err(invalid("use a positive amount, credit or debit, a request UUID and a reason of at most 500 characters"));
    }
    let digest =
        hash(&json!({"kind":request.kind,"amount_micros":request.amount_micros,"reason":reason}));
    let mut tx = state.pool.begin().await?;
    administrator(&state, &mut tx, &principal, org).await?;
    target(&mut tx, org, user).await?;
    let row = lock_wallet(&mut tx, org, user).await?;
    if let Some(previous)=sqlx::query_scalar::<_,String>("SELECT request_sha256 FROM management_wallet_ledger WHERE organisation_id=$1 AND user_id=$2 AND request_id=$3").bind(org).bind(user).bind(request.request_id).fetch_optional(&mut *tx).await? {
        if previous!=digest{return Err(ApiError::Conflict("request UUID already belongs to a different wallet entry".into()));}
    }else{
        if row.get::<i64,_>("revision")!=request.expected_revision{return Err(conflict());}
        post_ledger(&mut tx,org,user,request.request_id,&request.kind,request.amount_micros,reason,Some(principal.user_id),None,&digest).await?;
        append_audit(&mut tx,org,principal.user_id,"wallet.entry.created",user,json!({"request_id":request.request_id,"kind":request.kind,"amount_micros":request.amount_micros,"reason":reason})).await?;
    }
    let result = current(&mut tx, org, user, true).await?;
    tx.commit().await?;
    Ok(Json(result))
}

pub(crate) async fn reserve(
    state: &AppState,
    request: WalletReservationRequest,
) -> Result<WalletReservationDecision, ApiError> {
    if request.request_id.is_nil()
        || !hash_valid(&request.request_sha256)
        || !metadata_valid(&request.metadata)
    {
        return Err(invalid("invalid reservation request"));
    }
    if let Some(maximum) = request.maximum_micros {
        micros(maximum)?;
    }
    let mut tx = state.pool.begin().await?;
    organisation_lifecycle::share_active(&mut tx, request.organisation_id).await?;
    target(&mut tx, request.organisation_id, request.user_id).await?;
    let granted:bool=sqlx::query_scalar("SELECT management_resource_scope_available($1,$2,$3,NULL,$4) AND EXISTS(SELECT 1 FROM management_members WHERE organisation_id=$1 AND user_id=$2 AND role IN ('admin','member')) AND EXISTS(SELECT 1 FROM management_resource_assignments a WHERE a.organisation_id=$1 AND a.user_id=$2 AND a.resource_id=$4 AND a.environment_id IS NULL AND (a.instance_id IS NULL OR a.instance_id=$3))")
        .bind(request.organisation_id).bind(request.user_id).bind(request.instance_id).bind(request.model_resource_id).fetch_one(&mut *tx).await?;
    if !granted {
        return Err(denied());
    }
    let row = lock_wallet(&mut tx, request.organisation_id, request.user_id).await?;
    // Always detect an existing request before considering a later disable.
    if let Some(old)=sqlx::query("SELECT id,state,maximum_micros,request_sha256,model_resource_id,currency FROM management_wallet_reservations WHERE organisation_id=$1 AND instance_id=$2 AND user_id=$3 AND request_id=$4")
        .bind(request.organisation_id).bind(request.instance_id).bind(request.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?{
        if old.get::<String,_>("request_sha256")!=request.request_sha256||old.get::<Uuid,_>("model_resource_id")!=request.model_resource_id||Some(old.get::<i64,_>("maximum_micros"))!=request.maximum_micros||Some(old.get::<String,_>("currency"))!=request.currency{return Err(ApiError::Conflict("reservation request UUID was reused with different data".into()));}
        let reservation=WalletReservation{id:old.get("id"),state:old.get("state"),maximum_micros:old.get("maximum_micros")};
        tx.commit().await?;return Ok(WalletReservationDecision{enforced:true,reservation:Some(reservation),replayed:true});
    }
    if !row.get::<bool, _>("enabled") {
        tx.commit().await?;
        return Ok(WalletReservationDecision {
            enforced: false,
            reservation: None,
            replayed: false,
        });
    }
    let maximum = request.maximum_micros.ok_or_else(|| {
        ApiError::Unprocessable(
            "this member's wallet requires explicit bounded model pricing".into(),
        )
    })?;
    if request.currency.as_deref() != Some(row.get::<String, _>("currency").as_str()) {
        return Err(ApiError::Unprocessable(
            "model tariff currency must match the organisation wallet currency".into(),
        ));
    }
    let (reserved, monthly_reserved, spent) =
        totals(&mut tx, request.organisation_id, request.user_id).await?;
    let monthly_limit = row
        .get::<Option<i64>, _>("monthly_limit_micros")
        .unwrap_or(MAX_MICROS);
    if maximum > row.get::<i64, _>("balance_micros") - reserved
        || maximum > monthly_limit - spent - monthly_reserved
    {
        return Err(ApiError::Quota(
            "member wallet balance or monthly API budget is exhausted".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO management_wallet_reservations(id,organisation_id,user_id,instance_id,model_resource_id,request_id,request_sha256,currency,maximum_micros,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(id).bind(request.organisation_id).bind(request.user_id).bind(request.instance_id).bind(request.model_resource_id).bind(request.request_id).bind(request.request_sha256).bind(request.currency).bind(maximum).bind(request.metadata).execute(&mut *tx).await?;
    sqlx::query("UPDATE management_member_wallets SET revision=revision+1,updated_at=clock_timestamp() WHERE organisation_id=$1 AND user_id=$2").bind(request.organisation_id).bind(request.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(WalletReservationDecision {
        enforced: true,
        reservation: Some(WalletReservation {
            id,
            state: "reserved".into(),
            maximum_micros: maximum,
        }),
        replayed: false,
    })
}

async fn reservation_scope(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(Uuid, Uuid), ApiError> {
    let row = sqlx::query(
        "SELECT organisation_id,user_id FROM management_wallet_reservations WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)?;
    let org = row.get("organisation_id");
    let user = row.get("user_id");
    organisation_lifecycle::share_active(tx, org).await?;
    lock_wallet(tx, org, user).await?;
    Ok((org, user))
}
async fn settle_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    actual: i64,
    receipt: &str,
    metadata: &Value,
    actor: Option<Uuid>,
    reason: &str,
) -> Result<(), ApiError> {
    micros(actual)?;
    if !hash_valid(receipt) || !metadata_valid(metadata) {
        return Err(invalid("invalid settlement receipt"));
    }
    let (org, user) = reservation_scope(tx, id).await?;
    let row = sqlx::query("SELECT * FROM management_wallet_reservations WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if row.get::<String, _>("state") == "settled" {
        if row.get::<Option<i64>, _>("charged_micros") != Some(actual)
            || row.get::<Option<String>, _>("receipt_sha256").as_deref() != Some(receipt)
        {
            return Err(ApiError::Conflict(
                "reservation already settled with a different receipt".into(),
            ));
        }
        return Ok(());
    }
    if actual > row.get::<i64, _>("maximum_micros") {
        return Err(ApiError::Conflict(
            "usage exceeds the authorised reservation; hold it for investigation".into(),
        ));
    }
    // Release this reservation in the same transaction as its unique ledger debit.
    sqlx::query("UPDATE management_wallet_reservations SET state='settled',charged_micros=$2,receipt_sha256=$3,metadata=metadata||jsonb_build_object('settlement',$4::jsonb),updated_at=clock_timestamp() WHERE id=$1")
        .bind(id).bind(actual).bind(receipt).bind(metadata).execute(&mut **tx).await?;
    post_ledger(
        tx,
        org,
        user,
        id,
        "usage_charge",
        actual,
        reason,
        actor,
        Some(id),
        receipt,
    )
    .await?;
    Ok(())
}
pub(crate) async fn settle(
    state: &AppState,
    reservation_id: Uuid,
    actual_micros: i64,
    receipt_sha256: &str,
    metadata: Value,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    settle_in_tx(
        &mut tx,
        reservation_id,
        actual_micros,
        receipt_sha256,
        &metadata,
        None,
        "Verified central model tariff usage",
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn hold(
    state: &AppState,
    reservation_id: Uuid,
    reason_code: &str,
) -> Result<(), ApiError> {
    if reason_code.is_empty()
        || reason_code.len() > 120
        || !reason_code
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'-'))
    {
        return Err(invalid("invalid reservation hold reason"));
    }
    let mut tx = state.pool.begin().await?;
    let (org, user) = reservation_scope(&mut tx, reservation_id).await?;
    let changed=sqlx::query("UPDATE management_wallet_reservations SET state='held',held_reason=$2,updated_at=clock_timestamp() WHERE id=$1 AND state='reserved'").bind(reservation_id).bind(reason_code).execute(&mut *tx).await?.rows_affected();
    if changed > 0 {
        sqlx::query("UPDATE management_member_wallets SET revision=revision+1,updated_at=clock_timestamp() WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn resolve(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, user, id)): Path<(Uuid, Uuid, Uuid)>,
    Json(request): Json<Resolve>,
) -> Result<Json<Value>, ApiError> {
    micros(request.actual_micros)?;
    let reason = request.reason.trim();
    if request.request_id.is_nil()
        || request.expected_revision < 1
        || reason.is_empty()
        || reason.chars().count() > 500
    {
        return Err(invalid("administrative resolution needs a request UUID, current revision and documented billing evidence"));
    }
    let receipt = hash(
        &json!({"admin_resolution":request.request_id,"actual_micros":request.actual_micros,"reason":reason,"reservation_id":id}),
    );
    let mut tx = state.pool.begin().await?;
    administrator(&state, &mut tx, &principal, org).await?;
    let wallet = lock_wallet(&mut tx, org, user).await?;
    let row=sqlx::query("SELECT state,receipt_sha256,(state='held' OR (state='reserved' AND created_at<clock_timestamp()-interval '10 minutes')) AS resolvable FROM management_wallet_reservations WHERE id=$1 AND organisation_id=$2 AND user_id=$3 FOR UPDATE").bind(id).bind(org).bind(user).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    let replay = row.get::<String, _>("state") == "settled"
        && row.get::<Option<String>, _>("receipt_sha256").as_deref() == Some(&receipt);
    if !replay {
        if wallet.get::<i64, _>("revision") != request.expected_revision {
            return Err(conflict());
        }
        if !row.get::<bool, _>("resolvable") {
            return Err(ApiError::Conflict(
                "only a held reservation or an interrupted reservation older than ten minutes can be administratively resolved".into(),
            ));
        }
    }
    settle_in_tx(
        &mut tx,
        id,
        request.actual_micros,
        &receipt,
        &json!({"admin_resolution":request.request_id,"reason":reason}),
        Some(principal.user_id),
        reason,
    )
    .await?;
    if !replay {
        append_audit(&mut tx,org,principal.user_id,"wallet.reservation.resolved",id,json!({"actual_micros":request.actual_micros,"reason":reason,"request_id":request.request_id})).await?;
    }
    let result = current(&mut tx, org, user, true).await?;
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn settings(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<SettingsUpdate>,
) -> Result<Json<Value>, ApiError> {
    if request.expected_revision < 1
        || request.currency.len() != 3
        || !request.currency.bytes().all(|v| v.is_ascii_uppercase())
    {
        return Err(invalid(
            "use a three-letter uppercase currency and current settings revision",
        ));
    }
    let mut tx = state.pool.begin().await?;
    administrator(&state, &mut tx, &principal, org).await?;
    let row=sqlx::query("SELECT currency,revision FROM management_wallet_settings WHERE organisation_id=$1 FOR UPDATE").bind(org).fetch_one(&mut *tx).await?;
    if row.get::<i64, _>("revision") != request.expected_revision {
        return Err(conflict());
    }
    let locked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_wallet_ledger WHERE organisation_id=$1) OR EXISTS(SELECT 1 FROM management_wallet_reservations WHERE organisation_id=$1)").bind(org).fetch_one(&mut *tx).await?;
    if locked && row.get::<String, _>("currency") != request.currency {
        return Err(ApiError::Conflict("currency is permanently locked after the first ledger entry or reservation; currency conversion is not supported".into()));
    }
    sqlx::query("UPDATE management_wallet_settings SET currency=$2,revision=revision+1,updated_by=$3,updated_at=clock_timestamp() WHERE organisation_id=$1").bind(org).bind(&request.currency).bind(principal.user_id).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "wallet.currency.changed",
        org,
        json!({"currency":request.currency}),
    )
    .await?;
    let result = json!({"organisation_id":org,"currency":request.currency,"revision":request.expected_revision+1,"currency_locked":locked});
    tx.commit().await?;
    Ok(Json(result))
}

#[cfg(test)]
#[path = "wallet_tests.rs"]
mod tests;
