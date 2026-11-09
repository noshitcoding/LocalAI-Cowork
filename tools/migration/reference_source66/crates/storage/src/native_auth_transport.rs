//! Native-only encrypted original requests, under the existing explicit native-key writer.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use sqlx::{Any, Transaction};
use zeroize::{Zeroize, Zeroizing};
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    request: NativeAuthTransportRequest,
    state: String,
    response: Option<String>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.request.body_json.zeroize();
        if let Some(v) = &mut self.response {
            v.zeroize();
        }
    }
}
fn invalid() -> Error {
    Error::Validation("native_auth_transport_invalid".into())
}
fn validate(v: &NativeAuthTransportRequest) -> Result<()> {
    if v.operation_id.is_nil()
        || v.endpoint.len() > 1024
        || v.body_json.len() > 1024
        || !matches!(v.purpose.as_str(), "native_token" | "registration_ack")
    {
        return Err(invalid());
    }
    let url = reqwest::Url::parse(&v.endpoint).map_err(|_| invalid())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.as_str() != v.endpoint
        || !(url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
                )))
    {
        return Err(invalid());
    }
    let body: AndroidNativeTokenExchange =
        serde_json::from_str(&v.body_json).map_err(|_| invalid())?;
    if body.code.len() != 69
        || !body.code.starts_with("cw2a_")
        || !body.code[5..].bytes().all(|b| b.is_ascii_hexdigit())
        || !(43..=128).contains(&body.code_verifier.len())
        || !body
            .code_verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        || Uuid::parse_str(&body.device_id).is_err()
    {
        return Err(invalid());
    }
    if (v.purpose == "registration_ack" && (v.actor_id.is_none() || v.session_id.is_none()))
        || (v.purpose == "native_token" && (v.actor_id.is_some() || v.session_id.is_some()))
        || v.actor_id.is_some_and(|v| v.is_nil())
        || v.session_id.is_some_and(|v| v.is_nil())
    {
        return Err(invalid());
    }
    Ok(())
}
fn request_sha(v: &NativeAuthTransportRequest) -> Result<String> {
    Ok(hash(
        json_string(&(
            v.operation_id,
            &v.endpoint,
            &v.purpose,
            &v.body_json,
            v.actor_id,
            v.session_id,
        ))?
        .as_bytes(),
    ))
}
fn scope(instance: Uuid, v: &NativeAuthTransportRequest, sha: &str) -> CredentialScope {
    CredentialScope {
        project_id: format!("native-auth-transport/{instance}"),
        server_id: v.operation_id.to_string(),
        reference: format!("original-request/v1:{sha}"),
        revision: 1,
        endpoint: v.endpoint.clone(),
    }
}
fn view(r: &sqlx::any::AnyRow) -> Result<NativeAuthTransportView> {
    Ok(NativeAuthTransportView {
        operation_id: Uuid::parse_str(
            &r.try_get::<String, _>("operation_id")
                .map_err(storage_error)?,
        )
        .map_err(|_| invalid())?,
        endpoint: r.try_get("endpoint").map_err(storage_error)?,
        purpose: r.try_get("purpose").map_err(storage_error)?,
        state: r.try_get("state").map_err(storage_error)?,
        actor_id: r
            .try_get::<Option<String>, _>("actor_id")
            .map_err(storage_error)?
            .map(|v| Uuid::parse_str(&v).map_err(|_| invalid()))
            .transpose()?,
        session_id: r
            .try_get::<Option<String>, _>("session_id")
            .map_err(storage_error)?
            .map(|v| Uuid::parse_str(&v).map_err(|_| invalid()))
            .transpose()?,
    })
}
async fn lock(tx: &mut Transaction<'_, Any>, expected: &NativeSecretState) -> Result<()> {
    let actual = crate::native_secrets::lock_state(tx).await?;
    if &actual != expected || actual.pending_operation.is_some() || actual.active.is_none() {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn seal(keys: &dyn KeyStore, key: KeyId, scope: &CredentialScope, p: &Payload) -> Result<String> {
    let bytes = Zeroizing::new(serde_json::to_vec(p).map_err(storage_error)?);
    let encoded = Zeroizing::new(STANDARD.encode(&*bytes));
    if encoded.len() > 4096 {
        return Err(Error::BudgetExceeded);
    }
    json_string(&seal_bearer(keys, key, scope, &encoded).map_err(|_| Error::NativeKeyUnavailable)?)
}
fn open(r: &sqlx::any::AnyRow, keys: &dyn KeyStore, instance: Uuid) -> Result<Payload> {
    if r.try_get::<String, _>("instance_id")
        .map_err(storage_error)?
        != instance.to_string()
    {
        return Err(Error::Forbidden);
    }
    let scope_saved: CredentialScope =
        parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
    let sealed: SealedCredential =
        parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    if sealed.key_id().instance != instance {
        return Err(Error::Forbidden);
    }
    let plain =
        open_bearer(keys, &scope_saved, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(
        STANDARD
            .decode(plain.as_bytes())
            .map_err(|_| Error::Forbidden)?,
    );
    let p: Payload = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
    validate(&p.request)?;
    let v = view(r)?;
    let sha = request_sha(&p.request)?;
    if p.request.operation_id != v.operation_id
        || p.request.endpoint != v.endpoint
        || p.request.purpose != v.purpose
        || p.request.actor_id != v.actor_id
        || p.request.session_id != v.session_id
        || p.state != v.state
        || r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != sha
        || scope_saved != scope(instance, &p.request, &sha)
        || (p.state == "prepared") != p.response.is_none()
    {
        return Err(Error::Forbidden);
    }
    Ok(p)
}
impl Store {
    pub async fn native_auth_transport_pending(&self) -> Result<Vec<NativeAuthTransportView>> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        let rows=sqlx::query("SELECT * FROM native_auth_transport_journal WHERE instance_id=$1 AND state IN('prepared','response_saved') ORDER BY created_at LIMIT 65").bind(state.instance.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 64 {
            return Err(Error::BudgetExceeded);
        }
        let result = rows.iter().map(view).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn native_auth_transport_prepare(
        &self,
        keys: &dyn KeyStore,
        state: &NativeSecretState,
        v: &NativeAuthTransportRequest,
    ) -> Result<NativeAuthTransportView> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        validate(v)?;
        let sha = request_sha(v)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        lock(&mut tx, state).await?;
        if let Some(r) =
            sqlx::query("SELECT * FROM native_auth_transport_journal WHERE operation_id=$1")
                .bind(v.operation_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            let p = open(&r, keys, state.instance)?;
            if p.request != *v {
                return Err(Error::Conflict);
            }
            let result = view(&r)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM native_auth_transport_journal WHERE state IN('prepared','response_saved')").fetch_one(&mut *tx).await.map_err(storage_error)?;
        if n >= 64 {
            return Err(Error::BudgetExceeded);
        }
        let key = state
            .active
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)?
            .key;
        crate::native_key_retirement::allow_cipher(
            &mut tx,
            &key.cipher_id().map_err(|_| Error::Forbidden)?,
        )
        .await?;
        let scope = scope(state.instance, v, &sha);
        let payload = Payload {
            request: v.clone(),
            state: "prepared".into(),
            response: None,
        };
        let sealed = seal(keys, key, &scope, &payload)?;
        let current = now();
        sqlx::query("INSERT INTO native_auth_transport_journal(operation_id,instance_id,endpoint,purpose,state,request_sha256,actor_id,session_id,scope,sealed,created_at,updated_at) VALUES($1,$2,$3,$4,'prepared',$5,$6,$7,$8,$9,$10,$10)")
            .bind(v.operation_id.to_string()).bind(state.instance.to_string()).bind(&v.endpoint).bind(&v.purpose).bind(sha).bind(v.actor_id.map(|v|v.to_string())).bind(v.session_id.map(|v|v.to_string())).bind(json_string(&scope)?).bind(sealed).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(NativeAuthTransportView {
            operation_id: v.operation_id,
            endpoint: v.endpoint.clone(),
            purpose: v.purpose.clone(),
            state: "prepared".into(),
            actor_id: v.actor_id,
            session_id: v.session_id,
        })
    }
    pub async fn native_auth_transport_read(
        &self,
        keys: &dyn KeyStore,
        state: &NativeSecretState,
        operation: Uuid,
    ) -> Result<NativeAuthTransportReplay> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        lock(&mut tx, state).await?;
        let r=sqlx::query("SELECT * FROM native_auth_transport_journal WHERE operation_id=$1 AND state IN('prepared','response_saved')").bind(operation.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let p = open(&r, keys, state.instance)?;
        crate::native_key_retirement::allow_cipher(
            &mut tx,
            &parse::<SealedCredential>(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?
                .key_id()
                .cipher_id()
                .map_err(|_| Error::Forbidden)?,
        )
        .await?;
        let result = NativeAuthTransportReplay {
            request: p.request.clone(),
            response_json: p.response.clone(),
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn native_auth_transport_capture(
        &self,
        keys: &dyn KeyStore,
        state: &NativeSecretState,
        operation: Uuid,
        response: &str,
    ) -> Result<NativeAuthTransportView> {
        if !self.sqlite || response.len() > 2048 {
            return Err(invalid());
        }
        let _: Value = parse(response)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        lock(&mut tx, state).await?;
        let r=sqlx::query("SELECT * FROM native_auth_transport_journal WHERE operation_id=$1 AND state IN('prepared','response_saved')").bind(operation.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let mut p = open(&r, keys, state.instance)?;
        if p.response.as_deref().is_some_and(|v| v != response) {
            return Err(Error::Conflict);
        }
        if p.response.is_some() {
            let v = view(&r)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        p.state = "response_saved".into();
        p.response = Some(response.into());
        let scope: CredentialScope =
            parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
        let key = state
            .active
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)?
            .key;
        crate::native_key_retirement::allow_cipher(
            &mut tx,
            &key.cipher_id().map_err(|_| Error::Forbidden)?,
        )
        .await?;
        let sealed = seal(keys, key, &scope, &p)?;
        if sqlx::query("UPDATE native_auth_transport_journal SET state='response_saved',sealed=$1,updated_at=$2 WHERE operation_id=$3 AND instance_id=$4 AND state='prepared' AND sealed=$5 AND scope=$6").bind(sealed).bind(now()).bind(operation.to_string()).bind(state.instance.to_string()).bind(r.try_get::<String,_>("sealed").map_err(storage_error)?).bind(json_string(&scope)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        let mut v = view(&r)?;
        v.state = "response_saved".into();
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn native_auth_transport_finish(
        &self,
        operation: Uuid,
        acknowledged: bool,
    ) -> Result<NativeAuthTransportView> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        let r = sqlx::query(
            "SELECT * FROM native_auth_transport_journal WHERE operation_id=$1 AND instance_id=$2",
        )
        .bind(operation.to_string())
        .bind(state.instance.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let mut v = view(&r)?;
        let next = if acknowledged {
            "acknowledged"
        } else {
            "abandoned"
        };
        if v.state == next {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if !matches!(v.state.as_str(), "prepared" | "response_saved")
            || acknowledged && v.state != "response_saved"
        {
            return Err(Error::Conflict);
        }
        if sqlx::query("UPDATE native_auth_transport_journal SET state=$1,scope='',sealed='',updated_at=$2 WHERE operation_id=$3 AND instance_id=$4 AND state=$5").bind(next).bind(now()).bind(operation.to_string()).bind(state.instance.to_string()).bind(&v.state).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        v.state = next.into();
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
}
pub(crate) async fn required_keys(
    tx: &mut Transaction<'_, Any>,
    instance: Uuid,
) -> Result<Vec<KeyId>> {
    let rows=sqlx::query("SELECT scope,sealed FROM native_auth_transport_journal WHERE instance_id=$1 AND state IN('prepared','response_saved') LIMIT 65").bind(instance.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 64 {
        return Err(Error::BudgetExceeded);
    }
    let mut result = Vec::new();
    for r in rows {
        let sealed: SealedCredential =
            parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
        if sealed.key_id().instance != instance {
            return Err(Error::Forbidden);
        }
        if !result.contains(&sealed.key_id()) {
            result.push(sealed.key_id());
        }
    }
    Ok(result)
}
pub(crate) async fn rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let rows=sqlx::query("SELECT * FROM native_auth_transport_journal WHERE instance_id=$1 AND state IN('prepared','response_saved') ORDER BY operation_id LIMIT 65").bind(target.instance.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 64 {
        return Err(Error::BudgetExceeded);
    }
    let mut count = 0;
    for r in rows {
        let p = open(&r, keys, target.instance)?;
        let raw: String = r.try_get("sealed").map_err(storage_error)?;
        let old: SealedCredential = parse(&raw)?;
        if old.key_id() == target {
            continue;
        }
        let scope: CredentialScope =
            parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
        let sealed = seal(keys, target, &scope, &p)?;
        if sqlx::query("UPDATE native_auth_transport_journal SET sealed=$1 WHERE operation_id=$2 AND instance_id=$3 AND request_sha256=$4 AND scope=$5 AND sealed=$6 AND state=$7").bind(sealed).bind(r.try_get::<String,_>("operation_id").map_err(storage_error)?).bind(target.instance.to_string()).bind(r.try_get::<String,_>("request_sha256").map_err(storage_error)?).bind(json_string(&scope)?).bind(raw).bind(r.try_get::<String,_>("state").map_err(storage_error)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        count += 1;
    }
    Ok(count)
}
