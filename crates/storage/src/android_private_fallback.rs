//! A human review can skip a proposed action, never approve or replay its effect.
use super::*;
impl Store {
    pub async fn android_private_loop_human_fallback(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<bool> {
        let key = workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, stamp).await?;
        let mut cp = checkpoint(&mut tx, &r, keys).await?;
        loop_authority(self, &mut tx, &r, &cp, stamp).await?;
        let spec: RunSpec =
            serde_json::from_value(cp["spec"].clone()).map_err(|_| device_invalid())?;
        let applies = spec
            .agent_config
            .as_ref()
            .and_then(|c| c.behavior.as_ref())
            .is_some_and(|b| b.fallback_to_human_on_repeated_failure)
            && cowork_domain::consecutive_tool_failures(
                cp["messages"].as_array().ok_or(Error::Forbidden)?,
            ) >= 3;
        if !applies || cp["pending_tools"].as_array().is_none_or(Vec::is_empty) {
            tx.commit().await.map_err(storage_error)?;
            return Ok(false);
        }
        let tool = pending(&cp)?;
        if tool.name == "request_input" {
            tx.commit().await.map_err(storage_error)?;
            return Ok(false);
        }
        let req = request(&mut tx, &r, &cp, &tool, "input", stamp).await?;
        if chrono::DateTime::parse_from_rfc3339(req["expires_at"].as_str().ok_or(Error::Forbidden)?)
            .map_err(|_| Error::Forbidden)?
            .timestamp_millis()
            <= stamp
        {
            return Err(Error::Forbidden);
        }
        if req["state"] == "submitted" {
            let result = json!({"effect_dispatched":false,"skipped":true,"reason":"repeated_tool_failure","human_response":req["response"],"request_id":req["id"]});
            complete_tool(&mut tx, &r, &mut cp, &tool, &result, keys, key, stamp).await?;
            live_after_crypto(self, &mut tx, &r, &cp).await?;
        } else if req["state"] != "pending" {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(true)
    }
}
