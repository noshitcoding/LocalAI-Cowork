use super::*;
pub(crate) async fn check(tx: &mut sqlx::Transaction<'_, sqlx::Any>, instance: Uuid) -> Result<()> {
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_oidc_exchanges e JOIN android_oidc_flows f ON f.id=e.flow_id JOIN android_oidc_ciphertext c ON c.id=f.payload_id WHERE e.producer_finished_at=0 AND c.key_instance=$1")
        .bind(instance.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 0 {
        return Err(Error::UncertainEffect);
    }
    Ok(())
}
