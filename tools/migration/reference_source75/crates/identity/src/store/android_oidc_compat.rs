//! Existing V1 clients receive a server native code bound to their original outer PKCE.
use super::*;
use cowork_contracts::{AndroidOidcCompatStart, AndroidOidcFinish};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatOriginal {
    request: AndroidOidcCompatStart,
    begin: AndroidOidcBegin,
    provider_state: String,
    finish_id: Uuid,
    operator_revision: i64,
}
impl Drop for CompatOriginal {
    fn drop(&mut self) {
        self.begin.resume_secret.zeroize();
    }
}
fn outer(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || b"-_".contains(&v))
}
impl IdentityStore {
    pub async fn android_oidc_compat_config(
        &self,
        organization: Option<Uuid>,
        server_callback: Option<&str>,
    ) -> Result<Value> {
        let mut actual = self
            .android_oidc_profiles(None, organization, false)
            .await?;
        if let Some(org) = organization {
            let mut tx = self.begin().await?;
            let policy = sso::read(&mut tx, org).await?;
            actual.retain(|p| policy.enabled && policy.profile == Some(p.id));
            tx.commit().await?;
        }
        let enabled = actual.len() == 1
            && server_callback
                .is_some_and(|url| actual[0].config.redirect_uris.iter().any(|u| u == url));
        if let Some(id) = organization {
            let mut tx = self.begin().await?;
            let name: String = sqlx::query_scalar(
                "SELECT name FROM identity_organizations WHERE id=$1 AND state='active'",
            )
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(IdentityError::NotFound)?;
            profiles::org_epoch(&mut tx, organization).await?;
            tx.commit().await?;
            Ok(
                json!({"enabled": enabled,"organisation_id":id,"organisation_name":name,"provider_label":if enabled { Some(actual[0].name.clone()) } else { None }}),
            )
        } else {
            Ok(json!({"schema_version":2,"enabled":enabled}))
        }
    }
    pub async fn android_oidc_compat_start(
        &self,
        actor: Option<&Principal>,
        organization: Option<Uuid>,
        q: &AndroidOidcCompatStart,
        server_callback: &str,
        operator_revision: i64,
        keys: Arc<dyn KeyStore>,
        key: KeyId,
    ) -> Result<AndroidOidcFlow> {
        if q.code_challenge_method != "S256"
            || !outer(&q.code_challenge, 43, 43)
            || !outer(&q.client_state, 43, 128)
            || operator_revision < 1
        {
            return Err(IdentityError::Validation);
        }
        let mut profiles = self
            .android_oidc_profiles(None, organization, false)
            .await?;
        if let Some(org) = organization {
            let mut tx = self.begin().await?;
            let policy = sso::read(&mut tx, org).await?;
            profiles.retain(|p| policy.enabled && policy.profile == Some(p.id));
            tx.commit().await?;
        }
        // Legacy requests have no profile selector. Ambiguous realms cannot choose a provider implicitly.
        if profiles.len() != 1
            || !profiles[0]
                .config
                .redirect_uris
                .iter()
                .any(|u| u == server_callback)
        {
            return Err(IdentityError::Forbidden);
        }
        let begin = AndroidOidcBegin {
            request_id: Uuid::new_v4(),
            profile_id: profiles[0].id,
            device_id: q.device_id.clone(),
            redirect_uri: server_callback.into(),
            purpose: if actor.is_some() { "link" } else { "login" }.into(),
            resume_secret: random(),
            native_code_challenge: Some(q.code_challenge.clone()),
        };
        let result = self
            .android_oidc_begin(actor, &begin, keys.clone(), key)
            .await?;
        let provider_state = reqwest::Url::parse(&result.authorization_url)
            .map_err(|_| IdentityError::Corrupt)?
            .query_pairs()
            .find(|(name, _)| name == "state")
            .map(|(_, value)| value.into_owned())
            .ok_or(IdentityError::Corrupt)?;
        let original = CompatOriginal {
            request: q.clone(),
            begin,
            provider_state,
            finish_id: Uuid::new_v4(),
            operator_revision,
        };
        let mut tx = self.begin().await?;
        let f = flow(&mut tx, result.id, &original.begin.resume_secret).await?;
        let profile = self.current_oidc_flow(&mut tx, &f, false).await?;
        if profile.organization_id != organization
            || f.try_get::<String, _>("state_sha256")?
                != secret_hash("oidc-state", &original.provider_state)
        {
            return Err(IdentityError::Corrupt);
        }
        let at = self.sample(&mut tx).await?;
        let payload =
            Zeroizing::new(serde_json::to_vec(&original).map_err(|_| IdentityError::Corrupt)?);
        let record = cipher::put(
            &mut tx,
            keys.as_ref(),
            key,
            &profile,
            Uuid::new_v4(),
            "oauth_flow",
            &payload,
            at,
        )
        .await?;
        self.current_oidc_flow(&mut tx, &f, false).await?;
        sqlx::query("INSERT INTO android_oidc_compat_flows(flow_id,state_sha256,payload_id,operator_revision,created_at) VALUES($1,$2,$3,$4,$5)").bind(result.id.to_string()).bind(secret_hash("oidc-state", &original.provider_state)).bind(record.to_string()).bind(operator_revision).bind(at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// The callback request is reconstructed from encrypted original custody;
    /// it still must pass the actual SDK token exchange and verifier.
    pub async fn android_oidc_compat_callback(
        &self,
        state: &str,
        code: &str,
        issuer: Option<&str>,
        operator_revision: i64,
        keys: Arc<dyn KeyStore>,
    ) -> Result<(AndroidOidcFinish, String, String)> {
        if state.is_empty() || state.len() > 512 || code.is_empty() || code.len() > 8192 {
            return Err(IdentityError::Unauthenticated);
        }
        let mut tx = self.begin().await?;
        let row = sqlx::query("SELECT * FROM android_oidc_compat_flows WHERE state_sha256=$1 AND operator_revision=$2").bind(secret_hash("oidc-state", state)).bind(operator_revision).fetch_optional(&mut *tx).await?.ok_or(IdentityError::Unauthenticated)?;
        let id = identifier(row.try_get("flow_id")?)?;
        let f = sqlx::query("SELECT * FROM android_oidc_flows WHERE id=$1")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        let profile = self
            .current_oidc_flow(&mut tx, &f, f.try_get::<String, _>("phase")? == "committed")
            .await?;
        if issuer.is_some_and(|u| u != profile.config.issuer)
            || f.try_get::<String, _>("phase")? == "retired"
        {
            return Err(IdentityError::Unauthenticated);
        }
        let original: CompatOriginal = serde_json::from_slice(
            &cipher::read(
                &mut tx,
                keys.as_ref(),
                identifier(row.try_get("payload_id")?)?,
                profile.id,
                "oauth_flow",
            )
            .await?,
        )
        .map_err(|_| IdentityError::Corrupt)?;
        if original.provider_state != state
            || original.operator_revision != operator_revision
            || original.begin.profile_id != profile.id
            || f.try_get::<String, _>("resume_sha256")?
                != secret_hash("oidc-resume", &original.begin.resume_secret)
            || original.begin.native_code_challenge.as_ref()
                != Some(&original.request.code_challenge)
        {
            return Err(IdentityError::Corrupt);
        }
        let finish = AndroidOidcFinish {
            request_id: original.finish_id,
            flow: AndroidOidcComplete {
                flow_id: id,
                resume_secret: original.begin.resume_secret.clone(),
                state: state.into(),
                code: code.into(),
            },
            device_id: original.request.device_id.clone(),
            native_code_challenge: Some(original.request.code_challenge.clone()),
        };
        self.current_oidc_flow(&mut tx, &f, f.try_get::<String, _>("phase")? == "committed")
            .await?;
        tx.commit().await?;
        Ok((
            finish,
            original.request.redirect_uri.clone(),
            original.request.client_state.clone(),
        ))
    }
}
