use super::*;
pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/secrets", get(status).post(setup))
        .route("/secrets/requests", get(lookup))
        .route("/secrets/reconcile", post(reconcile))
        .route("/secrets/abandon", post(abandon))
        .route("/secrets/backup", post(backup))
        .route("/secrets/restore", post(restore))
        .route("/secrets/reencrypt", post(reencrypt))
        .route("/secrets/cleanup", get(cleanup).post(retire))
        .route("/secrets/orphans/retire", post(retire_orphan))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupInput {
    expected_revision: i64,
    password: zeroize::Zeroizing<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreInput {
    expected_revision: i64,
    password: zeroize::Zeroizing<String>,
    backup: cowork_secrets::KeyBackup,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReencryptInput {
    expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetireInput {
    expected_revision: i64,
    key: cowork_secrets::KeyId,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrphanRetireInput {
    expected_revision: i64,
    operation_id: String,
    idempotency_key: String,
}
async fn retire_orphan(State(s): State<AppState>, body: Input<OrphanRetireInput>) -> Answer<Value> {
    let selected = input(body)?;
    Ok(Json(
        s.runtime
            .retire_native_orphan_key(
                selected.expected_revision,
                &selected.operation_id,
                &selected.idempotency_key,
            )
            .await?,
    ))
}
async fn cleanup(State(s): State<AppState>) -> Answer<Value> {
    Ok(Json(s.runtime.native_key_cleanup().await?))
}
async fn retire(State(s): State<AppState>, body: Input<RetireInput>) -> Answer<Value> {
    let selected = input(body)?;
    Ok(Json(
        s.runtime
            .retire_native_key(
                selected.expected_revision,
                selected.key,
                &selected.idempotency_key,
            )
            .await?,
    ))
}
async fn backup(
    State(s): State<AppState>,
    body: Input<BackupInput>,
) -> Answer<cowork_secrets::KeyBackup> {
    let input = input(body)?;
    Ok(Json(
        s.runtime
            .backup_native_keys(input.expected_revision, input.password)
            .await?,
    ))
}
async fn restore(State(s): State<AppState>, body: Input<RestoreInput>) -> Answer<Value> {
    let input = input(body)?;
    Ok(Json(
        s.runtime
            .restore_native_keys(input.expected_revision, input.backup, input.password)
            .await?,
    ))
}
async fn reencrypt(State(s): State<AppState>, body: Input<ReencryptInput>) -> Answer<Value> {
    let input = input(body)?;
    Ok(Json(
        s.runtime
            .reencrypt_native_keys(input.expected_revision)
            .await?,
    ))
}
async fn status(State(state): State<AppState>) -> Answer<NativeSecretsStatus> {
    Ok(Json(state.runtime.native_secrets_status().await?))
}
async fn setup(
    State(state): State<AppState>,
    body: Input<NativeSecretsSetup>,
) -> Answer<NativeSecretsReceipt> {
    Ok(Json(
        state.runtime.setup_native_secrets(&input(body)?).await?,
    ))
}
async fn reconcile(
    State(state): State<AppState>,
    body: Input<NativeSecretsResolution>,
) -> Answer<NativeSecretsReceipt> {
    Ok(Json(
        state
            .runtime
            .reconcile_native_secrets(&input(body)?)
            .await?,
    ))
}
async fn abandon(
    State(state): State<AppState>,
    body: Input<NativeSecretsResolution>,
) -> Answer<NativeSecretsReceipt> {
    Ok(Json(
        state.runtime.abandon_native_secrets(&input(body)?).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookupQuery {
    key: String,
}
async fn lookup(
    State(state): State<AppState>,
    Query(query): Query<LookupQuery>,
) -> Answer<NativeSecretsRequestLookup> {
    Ok(Json(
        state.runtime.native_secrets_request(&query.key).await?,
    ))
}
