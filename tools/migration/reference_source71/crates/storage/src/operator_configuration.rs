//! Installation operator configuration; never included in team/run exports.
use super::*;

impl Store {
    pub async fn operator_device_configuration(&self) -> Result<Option<(i64, String)>> {
        sqlx::query(
            "SELECT revision,payload FROM operator_device_configuration WHERE id='devices-v1'",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(|row| {
            Ok((
                row.try_get("revision").map_err(storage_error)?,
                row.try_get("payload").map_err(storage_error)?,
            ))
        })
        .transpose()
    }

    pub async fn save_operator_device_configuration(
        &self,
        expected: i64,
        payload: &str,
    ) -> Result<i64> {
        let next = expected
            .checked_add(1)
            .filter(|v| *v > 0)
            .ok_or(Error::Conflict)?;
        if payload.len() > 200_000 {
            return Err(Error::Validation(
                "Operator configuration is too large".into(),
            ));
        }
        let mut tx=self.pool.begin().await.map_err(storage_error)?;
        crate::android_devices::device_writer(&mut tx).await?;
        let envelope:Value=parse(payload)?;
        let parts=envelope["parts"].as_array().filter(|p|!p.is_empty()&&p.len()<=22).ok_or(Error::NativeKeyUnavailable)?;
        for part in parts {
            let sealed:cowork_secrets::SealedCredential=serde_json::from_value(part.clone()).map_err(|_|Error::NativeKeyUnavailable)?;
            super::server_keys::cipher_writer(&mut tx,sealed.key_id()).await?;
        }
        let changed = if expected == 0 {
            sqlx::query("INSERT INTO operator_device_configuration(id,revision,payload) VALUES('devices-v1',$1,$2) ON CONFLICT(id) DO NOTHING")
                .bind(next).bind(payload).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        } else {
            sqlx::query("UPDATE operator_device_configuration SET revision=$1,payload=$2 WHERE id='devices-v1' AND revision=$3")
                .bind(next).bind(payload).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        };
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(next)
    }
}
