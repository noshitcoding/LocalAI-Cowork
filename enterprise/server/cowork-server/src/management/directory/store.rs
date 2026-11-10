use super::*;
pub async fn register(state: &AppState) -> anyhow::Result<()> {
    for c in state.directory.values() {
        let mut tx = state.pool.begin().await?;
        let active: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR SHARE",
        )
        .bind(c.org)
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            tracing::warn!(organisation=%c.org,"archived or missing organisation: directory operator slot remains inactive");
            tx.commit().await?;
            continue;
        }
        let p = state
            .organisation_oidc
            .get(&c.provider)
            .ok_or_else(|| anyhow::anyhow!("directory provider is not configured"))?;
        anyhow::ensure!(
            p.directory_identity(c.org) == Some((c.issuer.as_str(), c.client.as_str())),
            "directory original provider identity/organisation mismatch"
        );
        let encrypted = seal(
            state,
            c.id,
            "credential",
            &json!({"fingerprint":c.fingerprint,"organisation_id":c.org,"kind":c.kind,"secret":c.credential}),
        )?;
        sqlx::query("INSERT INTO directory_connectors(id,organisation_id,provider_id,issuer,client_id,kind,fingerprint,revision,epoch,active,credential_ciphertext,credential_key,credential_nonce,credential_wrap_nonce) VALUES($1,$2,$3,$4,$5,$6,$7,1,$8,true,$9,$10,$11,$12) ON CONFLICT(id) DO NOTHING")
            .bind(c.id).bind(c.org).bind(&c.provider).bind(&c.issuer).bind(&c.client).bind(&c.kind).bind(&c.fingerprint).bind(Uuid::new_v4()).bind(encrypted.ciphertext).bind(encrypted.encrypted_data_key).bind(encrypted.nonce.as_slice()).bind(encrypted.wrap_nonce.as_slice()).execute(&mut *tx).await?;
        let matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_connectors WHERE id=$1 AND organisation_id=$2 AND provider_id=$3 AND issuer=$4 AND client_id=$5 AND kind=$6 AND fingerprint=$7 AND active)")
            .bind(c.id).bind(c.org).bind(&c.provider).bind(&c.issuer).bind(&c.client).bind(&c.kind).bind(&c.fingerprint).fetch_one(&mut *tx).await?;
        if !matches {
            tracing::warn!(connector=%c.id,"directory operator slot inactive or changed; use explicit local directory inspect/activate CAS; startup leaves it unchanged");
        }
        tx.commit().await?;
    }
    Ok(())
}
pub async fn run_cli(
    pool: &sqlx::PgPool,
    key: &crate::storage::SecretStore,
    catalog: &catalog::Registry,
    args: &[String],
) -> anyhow::Result<()> {
    let (command,id,expected)=match args {
        [cmd,id] if cmd=="inspect" => (cmd,id,None),
        [cmd,id,rev] if cmd=="activate"||cmd=="disable" => (cmd,id,Some(rev.parse::<i64>()?)),
        _=>anyhow::bail!("directory inspect <connector-uuid> | activate|disable <connector-uuid> <expected-revision>; uses configured private operator catalog"),
    };
    let id: Uuid = id.parse()?;
    let c = catalog
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("connector is not in this operator catalog"))?;
    let mut tx = pool.begin().await?;
    let _: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(c.org)
    .fetch_optional(&mut *tx)
    .await?;
    let r = sqlx::query("SELECT * FROM directory_connectors WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connector not registered"))?;
    anyhow::ensure!(
        r.get::<Uuid, _>("organisation_id") == c.org
            && r.get::<String, _>("provider_id") == c.provider
            && r.get::<String, _>("issuer") == c.issuer
            && r.get::<String, _>("client_id") == c.client
            && r.get::<String, _>("kind") == c.kind,
        "original directory slot cannot be reassigned"
    );
    let output = if let Some(expected) = expected {
        let request = sha(&json!([1, id, command, expected, c.fingerprint]));
        if let Some(old)=sqlx::query("SELECT request_sha256,receipt FROM directory_operator_operations WHERE connector_id=$1 AND expected_revision=$2").bind(id).bind(expected).fetch_optional(&mut *tx).await? {
            anyhow::ensure!(old.get::<String,_>("request_sha256")==request,"revision consumed by another original operation"); old.get("receipt")
        } else {
            anyhow::ensure!(expected>0 && r.get::<i64,_>("revision")==expected,"inspect current operator revision before changing");
            let epoch=Uuid::new_v4(); let operation=Uuid::new_v4(); let active=command=="activate";
            let encrypted=key.seal_for_directory(id,&serde_json::to_vec(&json!({"version":1,"connector":id,"purpose":"credential","value":{"fingerprint":c.fingerprint,"organisation_id":c.org,"kind":c.kind,"secret":c.credential}}))?)?;
            let next=expected.checked_add(1).ok_or_else(||anyhow::anyhow!("revision overflow"))?;
            sqlx::query("UPDATE directory_connectors SET fingerprint=$2,revision=$3,epoch=$4,active=$5,credential_ciphertext=$6,credential_key=$7,credential_nonce=$8,credential_wrap_nonce=$9,updated_at=now() WHERE id=$1 AND revision=$10")
                .bind(id).bind(&c.fingerprint).bind(next).bind(epoch).bind(active).bind(encrypted.ciphertext).bind(encrypted.encrypted_data_key).bind(encrypted.nonce.as_slice()).bind(encrypted.wrap_nonce.as_slice()).bind(expected).execute(&mut *tx).await?;
            // Every epoch change retires old directory-origin sessions, even when
            // activating a replacement credential. Local/foreign sessions survive.
            resources::revoke(&mut tx,id,None).await?;
            let receipt=json!({"version":1,"operation_id":operation,"connector_id":id,"organisation_id":c.org,"action":command,"expected_revision":expected,"revision":next,"epoch":epoch,"fingerprint":c.fingerprint,"active":active,"request_sha256":request});
            sqlx::query("INSERT INTO directory_operator_operations(id,connector_id,expected_revision,request_sha256,receipt) VALUES($1,$2,$3,$4,$5)").bind(operation).bind(id).bind(expected).bind(request).bind(&receipt).execute(&mut *tx).await?;
            append_audit(&mut tx,c.org,Uuid::nil(),"directory.operator_change",id,receipt.clone()).await?; receipt
        }
    } else {
        json!({"version":1,"connector_id":id,"organisation_id":c.org,"kind":c.kind,"revision":r.get::<i64,_>("revision"),"epoch":r.get::<Uuid,_>("epoch"),"active":r.get::<bool,_>("active"),"fingerprint":r.get::<String,_>("fingerprint"),"configured_fingerprint":c.fingerprint})
    };
    tx.commit().await?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &p).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',c.id,'organisation_id',c.organisation_id,'provider_id',c.provider_id,'issuer',c.issuer,'client_id',c.client_id,'kind',c.kind,'fingerprint',c.fingerprint,'operator_revision',c.revision,'operator_active',c.active,'settings',CASE WHEN d.connector_id IS NULL THEN NULL ELSE to_jsonb(d) END,'job',CASE WHEN j.connector_id IS NULL THEN NULL ELSE jsonb_build_object('phase',j.phase,'pages',j.completed_pages,'entries',j.completed_entries,'last_success_at',j.last_success_at,'last_error_code',j.last_error_code,'next_poll_at',j.next_poll_at) END,'resources',(SELECT count(*) FROM directory_resources r WHERE r.connector_id=c.id AND NOT r.deleted),'quarantined',(SELECT count(*) FROM directory_resources r WHERE r.connector_id=c.id AND r.quarantined_legacy)) FROM directory_connectors c LEFT JOIN directory_settings d ON d.connector_id=c.id LEFT JOIN directory_sync_jobs j ON j.connector_id=c.id WHERE c.organisation_id=$1 ORDER BY c.id")
        .bind(org).fetch_all(&mut *tx).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|mut r| {
            let id = r["id"].as_str().and_then(|s| Uuid::parse_str(s).ok());
            r["configured"] = json!(id
                .and_then(|id| state.directory.get(&id))
                .is_some_and(|c| r["fingerprint"] == c.fingerprint));
            r
        })
        .collect();
    tx.commit().await?;
    Ok(Json(
        json!({"version":1,"items":items,"scim_path":"/management/scim/v2/{connector-id}","legacy_logout_scope":"Only explicitly inventoried users; all their unclassified old sessions/codes, which may include local sign-ins. Classified local/other-provider sessions are preserved."}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsRequest {
    expected_revision: i64,
    state: String,
    adopt_provider_memberships: bool,
    provision_accounts: bool,
    retention_days: i32,
}
pub(super) async fn settings(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(r): Json<SettingsRequest>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(r.state.as_str(), "paused" | "preview" | "active")
        || !(1..=3650).contains(&r.retention_days)
        || r.expected_revision < 0
    {
        return Err(invalid("invalid directory settings"));
    }
    let c = state.directory.get(&id).ok_or_else(unavailable)?;
    if c.org != org {
        return Err(denied());
    }
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &p).await?;
    let registry:bool=sqlx::query_scalar("SELECT active AND fingerprint=$7 FROM directory_connectors WHERE id=$1 AND organisation_id=$2 AND provider_id=$3 AND issuer=$4 AND client_id=$5 AND kind=$6 FOR UPDATE")
        .bind(id).bind(org).bind(&c.provider).bind(&c.issuer).bind(&c.client).bind(&c.kind).bind(&c.fingerprint).fetch_optional(&mut *tx).await?.ok_or_else(unavailable)?;
    let request_sha = sha(&json!([
        1,
        id,
        org,
        p.user_id,
        r.expected_revision,
        r.state,
        r.adopt_provider_memberships,
        r.provision_accounts,
        r.retention_days,
        c.fingerprint
    ]));
    if let Some(old)=sqlx::query("SELECT actor_user_id,request_sha256,receipt FROM directory_settings_operations WHERE connector_id=$1 AND expected_revision=$2").bind(id).bind(r.expected_revision).fetch_optional(&mut *tx).await? {
        if old.get::<Uuid,_>("actor_user_id")!=p.user_id||old.get::<String,_>("request_sha256")!=request_sha{return Err(conflict());}
        let receipt=old.get("receipt");tx.commit().await?;return Ok(Json(receipt));
    }
    let provider = state
        .organisation_oidc
        .get(&c.provider)
        .ok_or_else(unavailable)?;
    if provider.directory_identity(org) != Some((c.issuer.as_str(), c.client.as_str())) {
        return Err(unavailable());
    }
    let sso_revision:Option<i64>=sqlx::query_scalar("SELECT revision FROM management_sso WHERE organisation_id=$1 AND enabled AND provider_id=$2 AND provider_fingerprint=$3 FOR SHARE").bind(org).bind(&c.provider).bind(&provider.fingerprint).fetch_optional(&mut *tx).await?;
    let sso_revision = sso_revision.ok_or_else(unavailable)?;
    let policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
    let policy_revision = policy["revision"].as_i64().ok_or_else(unavailable)?;
    let prior: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM directory_settings WHERE connector_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if prior.unwrap_or(0) != r.expected_revision || (!registry && r.state != "paused") {
        return Err(conflict());
    }
    if r.state == "active" {
        let inventory = inventory(&mut tx, c).await?;
        if !inventory["unclassified_sessions"]
            .as_array()
            .unwrap()
            .is_empty()
            || !inventory["unclassified_codes"]
                .as_array()
                .unwrap()
                .is_empty()
            || !inventory["uncontained_users"]
                .as_array()
                .unwrap()
                .is_empty()
        {
            return Err(ApiError::Conflict("prepare and explicitly confirm the exact legacy containment inventory before activating this connector".into()));
        }
    }
    let revision = r.expected_revision.checked_add(1).ok_or_else(conflict)?;
    let epoch = Uuid::new_v4();
    sqlx::query("INSERT INTO directory_settings(connector_id,revision,epoch,state,provider_fingerprint,sso_revision,policy_revision,adopt_provider_memberships,provision_accounts,retention_days) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(connector_id) DO UPDATE SET revision=$2,epoch=$3,state=$4,provider_fingerprint=$5,sso_revision=$6,policy_revision=$7,adopt_provider_memberships=$8,provision_accounts=$9,retention_days=$10,updated_at=now()")
        .bind(id).bind(revision).bind(epoch).bind(&r.state).bind(&provider.fingerprint).bind(sso_revision).bind(policy_revision).bind(r.adopt_provider_memberships).bind(r.provision_accounts).bind(r.retention_days).execute(&mut *tx).await?;
    resources::revoke(&mut tx, id, None).await?;
    append_audit(&mut tx,org,p.user_id,"directory.settings",id,json!({"revision":revision,"state":r.state,"epoch":epoch,"adopt_provider_memberships":r.adopt_provider_memberships,"provision_accounts":r.provision_accounts,"sso_revision":sso_revision,"policy_revision":policy_revision})).await?;
    let receipt = json!({"connector_id":id,"revision":revision,"epoch":epoch,"state":r.state,"fresh_source_required":true});
    sqlx::query("INSERT INTO directory_settings_operations(id,connector_id,expected_revision,actor_user_id,request_sha256,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(Uuid::new_v4()).bind(id).bind(r.expected_revision).bind(p.user_id).bind(request_sha).bind(&receipt).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn inventory(
    tx: &mut Transaction<'_, Postgres>,
    c: &catalog::Connector,
) -> Result<Value, ApiError> {
    // Take actual user locks in a separate command before the fresh inventory
    // SELECT. Its snapshot must not predate a waited-for legacy INSERT commit.
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT DISTINCT i.user_id FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=$2 AND i.binding_method<>'legacy_unverified' AND u.deleted_at IS NULL AND (EXISTS(SELECT 1 FROM management_members m WHERE m.organisation_id=$3 AND m.user_id=i.user_id AND m.sso_provider_id=$4) OR EXISTS(SELECT 1 FROM directory_resources r WHERE r.connector_id=$1 AND r.identity_id=i.id AND r.user_id=i.user_id)) ORDER BY i.user_id LIMIT 10001")
        .bind(c.id).bind(&c.issuer).bind(c.org).bind(&c.provider).fetch_all(&mut **tx).await?;
    if ids.len() > 10000 {
        return Err(ApiError::Quota(
            "complete legacy user inventory exceeds its bound".into(),
        ));
    }
    let _: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    let users:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('user_id',i.user_id,'identity_id',i.id,'display_name',u.display_name,'contained',EXISTS(SELECT 1 FROM directory_containment_users t WHERE t.connector_id=$1 AND t.user_id=i.user_id AND t.identity_id=i.id)) FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=$2 AND i.binding_method<>'legacy_unverified' AND u.deleted_at IS NULL AND (EXISTS(SELECT 1 FROM management_members m WHERE m.organisation_id=$3 AND m.user_id=i.user_id AND m.sso_provider_id=$4) OR EXISTS(SELECT 1 FROM directory_resources r WHERE r.connector_id=$1 AND r.identity_id=i.id AND r.user_id=i.user_id)) ORDER BY i.user_id,i.id LIMIT 10001 FOR UPDATE OF u,i")
        .bind(c.id).bind(&c.issuer).bind(c.org).bind(&c.provider).fetch_all(&mut **tx).await?;
    if users.len() > 10000 {
        return Err(ApiError::Quota(
            "containment inventory requires an operator-reviewed smaller connector scope".into(),
        ));
    }
    let ids: Vec<Uuid> = users
        .iter()
        .filter_map(|v| v["user_id"].as_str()?.parse().ok())
        .collect();
    let sessions:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'user_id',user_id,'created_at',created_at,'expires_at',expires_at) FROM auth_sessions WHERE user_id=ANY($1) AND auth_origin IS NULL AND revoked_at IS NULL AND expires_at>now() ORDER BY user_id,id LIMIT 10001 FOR UPDATE")
        .bind(&ids).fetch_all(&mut **tx).await?;
    let codes:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('code_hash',encode(code_hash,'hex'),'user_id',user_id,'expires_at',expires_at) FROM native_authorization_codes WHERE user_id=ANY($1) AND auth_origin IS NULL AND consumed_at IS NULL AND expires_at>now() ORDER BY user_id,code_hash LIMIT 10001 FOR UPDATE")
        .bind(&ids).fetch_all(&mut **tx).await?;
    if sessions.len() > 10000 || codes.len() > 10000 {
        return Err(ApiError::Quota(
            "containment inventory exceeds complete bound".into(),
        ));
    }
    let uncontained: Vec<Value> = users
        .iter()
        .filter(|u| u["contained"] != true)
        .cloned()
        .collect();
    Ok(
        json!({"scope":"user-wide-unclassified-logout","connector_id":c.id,"organisation_id":c.org,"provider_id":c.provider,"issuer":c.issuer,"users":users,"uncontained_users":uncontained,"unclassified_sessions":sessions,"unclassified_codes":codes}),
    )
}
pub(super) async fn prepare_containment(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &p).await?;
    let c = current(&state, &mut tx, id, true).await?;
    if c.connector.org != org {
        return Err(denied());
    }
    let inv = inventory(&mut tx, &c.connector).await?;
    let hash = sha(&inv);
    let plan = Uuid::new_v4();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(10);
    sqlx::query("INSERT INTO directory_containment_plans(id,connector_id,settings_revision,registry_epoch,settings_epoch,provider_fingerprint,sso_revision,policy_revision,inventory_sha256,inventory,requested_by,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(plan).bind(id).bind(c.settings_revision).bind(c.registry_epoch).bind(c.settings_epoch).bind(&c.provider_fingerprint).bind(c.sso_revision).bind(c.policy_revision).bind(&hash).bind(&inv).bind(p.user_id).bind(expires).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"version":1,"plan_id":plan,"inventory_sha256":hash,"expected_revision":c.settings_revision,"expires_at":expires,"inventory":inv,"acknowledgement":"user-wide-unclassified-logout"}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Confirm {
    expected_revision: i64,
    inventory_sha256: String,
    acknowledgement: String,
}
pub(super) async fn confirm_containment(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((org, id, plan)): Path<(Uuid, Uuid, Uuid)>,
    Json(r): Json<Confirm>,
) -> Result<Json<Value>, ApiError> {
    if r.acknowledgement != "user-wide-unclassified-logout" {
        return Err(invalid(
            "explicit user-wide unclassified-session acknowledgement is required",
        ));
    }
    let mut tx = state.pool.begin().await?;
    sso::authorize(&mut tx, org, &p).await?;
    let row = sqlx::query(
        "SELECT * FROM directory_containment_plans WHERE id=$1 AND connector_id=$2 FOR UPDATE",
    )
    .bind(plan)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    if row.get::<String, _>("inventory_sha256") != r.inventory_sha256
        || row.get::<i64, _>("settings_revision") != r.expected_revision
        || row.get::<Uuid, _>("requested_by") != p.user_id
    {
        return Err(conflict());
    }
    if let Some(receipt) = row.get::<Option<Value>, _>("receipt") {
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    let c = current(&state, &mut tx, id, true).await?;
    if c.connector.org != org
        || c.settings_revision != r.expected_revision
        || c.registry_epoch != row.get::<Uuid, _>("registry_epoch")
        || c.settings_epoch != row.get::<Uuid, _>("settings_epoch")
        || c.provider_fingerprint != row.get::<String, _>("provider_fingerprint")
        || c.sso_revision != row.get::<i64, _>("sso_revision")
        || c.policy_revision != row.get::<i64, _>("policy_revision")
        || row.get::<chrono::DateTime<chrono::Utc>, _>("expires_at") <= chrono::Utc::now()
    {
        return Err(conflict());
    }
    let inv = inventory(&mut tx, &c.connector).await?;
    if sha(&inv) != r.inventory_sha256 {
        return Err(conflict());
    }
    for u in inv["users"].as_array().unwrap() {
        let user: Uuid = u["user_id"]
            .as_str()
            .unwrap()
            .parse()
            .map_err(|_| unavailable())?;
        let identity: Uuid = u["identity_id"]
            .as_str()
            .unwrap()
            .parse()
            .map_err(|_| unavailable())?;
        sqlx::query("INSERT INTO directory_containment_users(connector_id,user_id,identity_id,plan_id) VALUES($1,$2,$3,$4) ON CONFLICT(connector_id,user_id,identity_id) DO UPDATE SET plan_id=$4,confirmed_at=now()")
            .bind(id).bind(user).bind(identity).bind(plan).execute(&mut *tx).await?;
        sqlx::query("UPDATE auth_sessions SET revoked_at=now(),revoke_reason='directory_explicit_legacy_containment' WHERE user_id=$1 AND auth_origin IS NULL AND revoked_at IS NULL AND expires_at>now()").bind(user).execute(&mut *tx).await?;
        sqlx::query("UPDATE native_authorization_codes SET consumed_at=now() WHERE user_id=$1 AND auth_origin IS NULL AND consumed_at IS NULL AND expires_at>now()").bind(user).execute(&mut *tx).await?;
    }
    let receipt = json!({"version":1,"plan_id":plan,"connector_id":id,"organisation_id":org,"inventory_sha256":r.inventory_sha256,"scope":"user-wide-unclassified-logout","user_count":inv["users"].as_array().unwrap().len(),"session_count":inv["unclassified_sessions"].as_array().unwrap().len(),"code_count":inv["unclassified_codes"].as_array().unwrap().len(),"actor_user_id":p.user_id,"confirmed_at":chrono::Utc::now(),"classified_sessions_preserved":true});
    sqlx::query(
        "UPDATE directory_containment_plans SET receipt=$2 WHERE id=$1 AND receipt IS NULL",
    )
    .bind(plan)
    .bind(&receipt)
    .execute(&mut *tx)
    .await?;
    append_audit(
        &mut tx,
        org,
        p.user_id,
        "directory.explicit_legacy_containment",
        id,
        receipt.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
