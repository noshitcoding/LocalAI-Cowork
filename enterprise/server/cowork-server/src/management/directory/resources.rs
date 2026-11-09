use super::*;
#[derive(Clone)]
pub(super) struct Input {
    pub kind: String,
    pub external: String,
    pub subject: Option<String>,
    pub name: String,
    pub active: bool,
    pub deleted: bool,
    pub payload: Value,
    pub members: Vec<Uuid>,
    pub dn: Option<String>,
}
pub(super) async fn credential(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    c: &Current,
) -> Result<String, ApiError> {
    let r=sqlx::query("SELECT credential_ciphertext,credential_key,credential_nonce,credential_wrap_nonce FROM directory_connectors WHERE id=$1 AND fingerprint=$2 AND epoch=$3 AND active")
        .bind(c.connector.id).bind(&c.connector.fingerprint).bind(c.registry_epoch).fetch_optional(&mut **tx).await?.ok_or_else(unavailable)?;
    let v = open(
        state,
        c.connector.id,
        "credential",
        &sealed(&r, "credential")?,
    )?;
    if v["fingerprint"] != c.connector.fingerprint
        || v["organisation_id"] != json!(c.connector.org)
        || v["kind"] != c.connector.kind
    {
        return Err(unavailable());
    }
    v["secret"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(unavailable)
}
pub(super) async fn revoke(
    tx: &mut Transaction<'_, Postgres>,
    connector: Uuid,
    resource: Option<Uuid>,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE auth_sessions SET revoked_at=now(),revoke_reason='directory_authority_withdrawn' WHERE auth_origin->'directory'->>'connector_id'=$1 AND ($2::TEXT IS NULL OR auth_origin->'directory'->>'resource_id'=$2) AND revoked_at IS NULL")
        .bind(connector.to_string()).bind(resource.map(|id|id.to_string())).execute(&mut **tx).await?;
    sqlx::query("UPDATE native_authorization_codes SET consumed_at=now() WHERE auth_origin->'directory'->>'connector_id'=$1 AND ($2::TEXT IS NULL OR auth_origin->'directory'->>'resource_id'=$2) AND consumed_at IS NULL")
        .bind(connector.to_string()).bind(resource.map(|id|id.to_string())).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn replay(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    c: &Current,
    key: &str,
    hash: &str,
) -> Result<Option<Value>, ApiError> {
    if let Some(r)=sqlx::query("SELECT request_sha256,response_ciphertext,response_key,response_nonce,response_wrap_nonce FROM directory_operations WHERE connector_id=$1 AND request_key=$2")
        .bind(c.connector.id).bind(key).fetch_optional(&mut **tx).await? {
        if r.get::<String,_>("request_sha256")!=hash {return Err(conflict());}
        if r.get::<Option<Vec<u8>>,_>("response_ciphertext").is_none() {return Err(ApiError::Conflict("original directory response expired; its operation tombstone remains; inspect the current resource without replaying the mutation".into()));}
        return open(state,c.connector.id,&format!("response:{key}:{hash}"),&sealed(&r,"response")?).map(Some);
    }
    Ok(None)
}
pub(super) async fn journal(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    c: &Current,
    key: &str,
    hash: &str,
    id: Uuid,
    response: &Value,
) -> Result<(), ApiError> {
    let e = seal(
        state,
        c.connector.id,
        &format!("response:{key}:{hash}"),
        response,
    )?;
    let operation = Uuid::new_v4();
    let receipt = json!({"version":1,"operation_id":operation,"connector_id":c.connector.id,"resource_id":id,"request_sha256":hash,"source":c.stamp(),"response_sha256":sha(response)});
    sqlx::query("INSERT INTO directory_operations(id,connector_id,request_key,request_sha256,registry_epoch,settings_epoch,resource_id,receipt,response_ciphertext,response_key,response_nonce,response_wrap_nonce,retained_until) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,now()+make_interval(days=>$13))")
        .bind(operation).bind(c.connector.id).bind(key).bind(hash).bind(c.registry_epoch).bind(c.settings_epoch).bind(id).bind(&receipt).bind(e.ciphertext).bind(e.encrypted_data_key).bind(e.nonce.as_slice()).bind(e.wrap_nonce.as_slice()).bind(c.retention).execute(&mut **tx).await?;
    append_audit(
        tx,
        c.connector.org,
        Uuid::nil(),
        "directory.resource_operation",
        id,
        receipt,
    )
    .await?;
    Ok(())
}
pub(super) async fn payload(
    state: &AppState,
    row: &sqlx::postgres::PgRow,
    c: Uuid,
) -> Result<Value, ApiError> {
    if row
        .get::<Option<Vec<u8>>, _>("payload_ciphertext")
        .is_none()
    {
        return Err(ApiError::NotFound(
            "directory resource profile was retired".into(),
        ));
    }
    let id: Uuid = row.get("id");
    let v = open(
        state,
        c,
        &format!("resource:{id}"),
        &sealed(row, "payload")?,
    )?;
    if sha(&v) != row.get::<String, _>("payload_sha256") {
        return Err(unavailable());
    }
    Ok(v)
}
pub(super) async fn upsert(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    old: &Current,
    existing: Option<Uuid>,
    expected: Option<i64>,
    input: &Input,
    scan: Option<Uuid>,
) -> Result<Value, ApiError> {
    let c = recheck(state, tx, old).await?;
    let connector = c.connector.id;
    if input.external.is_empty()
        || input.external.len() > 512
        || input.name.is_empty()
        || input.name.len() > 512
        || input.members.len() > 1000
        || serde_json::to_vec(&input.payload)?.len() > 524288
    {
        return Err(invalid("directory resource exceeds bound"));
    }
    let previous = if let Some(id) = existing {
        sqlx::query("SELECT * FROM directory_resources WHERE connector_id=$1 AND id=$2 AND kind=$3 FOR UPDATE").bind(connector).bind(id).bind(&input.kind).fetch_optional(&mut **tx).await?
    } else {
        sqlx::query("SELECT * FROM directory_resources WHERE connector_id=$1 AND kind=$2 AND external_key=$3 FOR UPDATE").bind(connector).bind(&input.kind).bind(&input.external).fetch_optional(&mut **tx).await?
    };
    if existing.is_some() && previous.is_none() {
        return Err(ApiError::NotFound("directory resource not found".into()));
    }
    if let Some(p) = &previous {
        if p.get::<String, _>("external_key") != input.external
            || p.get::<Option<String>, _>("subject") != input.subject
        {
            return Err(ApiError::Conflict(
                "original directory externalId/subject cannot be reassigned".into(),
            ));
        }
        if expected.is_some_and(|r| r != p.get::<i64, _>("revision")) {
            return Err(conflict());
        }
        if p.get::<bool, _>("deleted") && !input.deleted {
            return Err(ApiError::Conflict(
                "retired directory identity cannot be silently reused".into(),
            ));
        }
    } else if expected.is_some() {
        return Err(conflict());
    }
    let id = previous.as_ref().map_or_else(Uuid::new_v4, |p| p.get("id"));
    let revision = previous
        .as_ref()
        .map_or(1, |p| p.get::<i64, _>("revision") + 1);
    let encrypted = seal(state, connector, &format!("resource:{id}"), &input.payload)?;
    sqlx::query("INSERT INTO directory_resources(id,connector_id,kind,external_key,subject,revision,active,deleted,name_hash,dn_hash,payload_sha256,payload_ciphertext,payload_key,payload_nonce,payload_wrap_nonce,observed_registry_epoch,observed_settings_epoch,observed_settings_revision,observed_scan,deleted_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,CASE WHEN $8 THEN now() ELSE NULL END) ON CONFLICT(id) DO UPDATE SET revision=$6,active=$7,deleted=$8,name_hash=$9,dn_hash=$10,payload_sha256=$11,payload_ciphertext=$12,payload_key=$13,payload_nonce=$14,payload_wrap_nonce=$15,observed_registry_epoch=$16,observed_settings_epoch=$17,observed_settings_revision=$18,observed_scan=$19,updated_at=now(),deleted_at=CASE WHEN $8 THEN coalesce(directory_resources.deleted_at,now()) ELSE NULL END")
        .bind(id).bind(connector).bind(&input.kind).bind(&input.external).bind(&input.subject).bind(revision).bind(input.active).bind(input.deleted).bind(hex::encode(Sha256::digest(input.name.as_bytes()))).bind(input.dn.as_ref().map(|s|hex::encode(Sha256::digest(s.as_bytes())))).bind(sha(&input.payload)).bind(encrypted.ciphertext).bind(encrypted.encrypted_data_key).bind(encrypted.nonce.as_slice()).bind(encrypted.wrap_nonce.as_slice()).bind(c.registry_epoch).bind(c.settings_epoch).bind(c.settings_revision).bind(scan).execute(&mut **tx).await?;
    if input.kind == "User" {
        let subject = input
            .subject
            .as_deref()
            .ok_or_else(|| invalid("User externalId must be the immutable OIDC subject"))?;
        if subject.is_empty() || subject.len() > 255 || !subject.is_ascii() {
            return Err(invalid("invalid original OIDC subject"));
        }
        let key = sha(&json!(["oidc-identity", c.connector.issuer, subject]));
        let bytes = hex::decode(key).map_err(|_| unavailable())?;
        let identity_lock = i64::from_be_bytes(bytes[..8].try_into().map_err(|_| unavailable())?);
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(identity_lock)
            .execute(&mut **tx)
            .await?;
        let known=sqlx::query("SELECT i.id,i.user_id,i.binding_method,i.directory_owner_connector_id,u.deleted_at FROM oidc_identities i JOIN users u ON u.id=i.user_id WHERE i.issuer=$1 AND i.subject=$2 FOR UPDATE OF i FOR SHARE OF u")
            .bind(&c.connector.issuer).bind(subject).fetch_optional(&mut **tx).await?;
        let mut identity = None;
        let mut user = None;
        let mut owned = false;
        if let Some(k) = known {
            if k.get::<Option<Uuid>, _>("directory_owner_connector_id")
                .is_some_and(|owner| owner != connector)
            {
                return Err(ApiError::Conflict(
                    "an identity owned by another directory connector cannot be adopted".into(),
                ));
            }
            if k.get::<String, _>("binding_method") == "legacy_unverified"
                || k.get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")
                    .is_some()
            {
                return Err(ApiError::Conflict("existing external identity needs its original authenticated binding; email cannot reconstruct it".into()));
            }
            identity = Some(k.get::<Uuid, _>("id"));
            user = Some(k.get::<Uuid, _>("user_id"));
            owned = k.get::<Option<Uuid>, _>("directory_owner_connector_id") == Some(connector);
        } else if c.state == "active" && c.provision && input.active && !input.deleted {
            let newuser = Uuid::new_v4();
            let newidentity = Uuid::new_v4();
            let email = input.payload["emails"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v["value"].as_str())
                .filter(|e| auth::validate_email_for_external_identity(e).is_ok())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("directory-{}@accounts.invalid", newuser.simple()));
            let created:Option<Uuid>=sqlx::query_scalar("INSERT INTO users(id,etag,email,display_name,password_hash,platform_admin) VALUES($1,$2,lower($3),$4,NULL,false) ON CONFLICT(lower(email)) WHERE deleted_at IS NULL DO NOTHING RETURNING id")
                .bind(newuser).bind(format!("W/\"{newuser}:1\"")).bind(&email).bind(&input.name).fetch_optional(&mut **tx).await?;
            if created.is_none() {
                return Err(ApiError::Conflict("account email already exists; explicitly link the original external identity, never adopt by email".into()));
            }
            sqlx::query("INSERT INTO oidc_identities(id,user_id,issuer,subject,email_at_link,binding_method,directory_owner_connector_id,directory_owner_resource_id) VALUES($1,$2,$3,$4,$5,'directory_provisioned',$6,$7)")
                .bind(newidentity).bind(newuser).bind(&c.connector.issuer).bind(subject).bind(email).bind(connector).bind(id).execute(&mut **tx).await?;
            user = Some(newuser);
            identity = Some(newidentity);
            owned = true;
        }
        let quarantined = if let (Some(u), Some(i)) = (user, identity) {
            !owned&&!sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM directory_containment_users WHERE connector_id=$1 AND user_id=$2 AND identity_id=$3)").bind(connector).bind(u).bind(i).fetch_one(&mut **tx).await?
        } else {
            false
        };
        sqlx::query("UPDATE directory_resources SET user_id=$2,identity_id=$3,owns_identity=$4,quarantined_legacy=$5 WHERE id=$1")
            .bind(id).bind(user).bind(identity).bind(owned).bind(quarantined).execute(&mut **tx).await?;
        if let (Some(u), Some(i)) = (user, identity) {
            sqlx::query("INSERT INTO directory_membership_contributions(connector_id,organisation_id,user_id,user_resource_id,identity_id,subject_sha256,provider_id,provider_fingerprint,sso_revision,policy_revision,registry_epoch,settings_epoch,settings_revision,role,active) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,NULL,$14) ON CONFLICT(connector_id,user_id,user_resource_id) DO UPDATE SET identity_id=$5,subject_sha256=$6,provider_id=$7,provider_fingerprint=$8,sso_revision=$9,policy_revision=$10,registry_epoch=$11,settings_epoch=$12,settings_revision=$13,active=$14,updated_at=now()")
                .bind(connector).bind(c.connector.org).bind(u).bind(id).bind(i).bind(hex::encode(Sha256::digest(subject.as_bytes()))).bind(&c.connector.provider).bind(&c.provider_fingerprint).bind(c.sso_revision).bind(c.policy_revision).bind(c.registry_epoch).bind(c.settings_epoch).bind(c.settings_revision).bind(c.state=="active"&&input.active&&!input.deleted&&!quarantined).execute(&mut **tx).await?;
            let role: Option<String> = sqlx::query_scalar("SELECT directory_effective_role($1,$2)")
                .bind(c.connector.org)
                .bind(u)
                .fetch_one(&mut **tx)
                .await?;
            // Manual/foreign-provider/content-admin rows are never adopted.
            if c.state == "active" && !quarantined {
                sqlx::query("INSERT INTO management_members(organisation_id,user_id,role,content_admin,sso_provider_id,directory_managed) VALUES($1,$2,$3,false,$4,true) ON CONFLICT(organisation_id,user_id) DO UPDATE SET directory_managed=true,sso_provider_id=$4,role=$3,content_admin=false WHERE management_members.directory_managed OR ($5 AND management_members.sso_provider_id=$4 AND NOT management_members.content_admin)")
                .bind(c.connector.org).bind(u).bind(role.unwrap_or_else(||"suspended".into())).bind(&c.connector.provider).bind(c.adopt&&c.state=="active"&&!quarantined).execute(&mut **tx).await?;
            }
            if !input.active || input.deleted || quarantined {
                revoke(tx, connector, Some(id)).await?;
            }
        }
    } else {
        sqlx::query("DELETE FROM directory_group_members WHERE connector_id=$1 AND group_id=$2")
            .bind(connector)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        if input.active && !input.deleted {
            for member in &input.members {
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_resources WHERE connector_id=$1 AND id=$2 AND kind='User' AND NOT deleted)").bind(connector).bind(member).fetch_one(&mut **tx).await?;
                if !valid {
                    return Err(invalid(
                        "group member must be an original User resource from this connector",
                    ));
                }
                sqlx::query("INSERT INTO directory_group_members(connector_id,group_id,user_resource_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(connector).bind(id).bind(member).execute(&mut **tx).await?;
            }
        }
        // Membership projection changes immediately; already issued provider
        // sessions also lose their original group grant on every group change.
        sqlx::query("UPDATE auth_sessions a SET revoked_at=now(),revoke_reason='directory_group_grant_withdrawn' WHERE a.auth_origin->'directory'->>'connector_id'=$1 AND a.revoked_at IS NULL AND directory_effective_role($2,a.user_id) IS NULL")
            .bind(connector.to_string()).bind(c.connector.org).execute(&mut **tx).await?;
        sqlx::query("UPDATE native_authorization_codes a SET consumed_at=now() WHERE a.auth_origin->'directory'->>'connector_id'=$1 AND a.consumed_at IS NULL AND directory_effective_role($2,a.user_id) IS NULL")
            .bind(connector.to_string()).bind(c.connector.org).execute(&mut **tx).await?;
    }
    let mut output = input.payload.clone();
    output["id"] = json!(id);
    output["externalId"] = json!(input.external);
    output["meta"] = json!({"resourceType":input.kind,"version":format!("W/\"{revision}\""),"lastModified":chrono::Utc::now()});
    Ok(output)
}
pub(super) async fn retain(state: &AppState) -> Result<(), ApiError> {
    // No identity, origin, contributor or once-operation hash is removed.
    sqlx::query("WITH expired AS (SELECT id FROM directory_operations WHERE retained_until<now() AND response_ciphertext IS NOT NULL ORDER BY id LIMIT 100 FOR UPDATE SKIP LOCKED) UPDATE directory_operations o SET response_ciphertext=NULL,response_key=NULL,response_nonce=NULL,response_wrap_nonce=NULL FROM expired e WHERE o.id=e.id")
        .execute(&state.pool).await?;
    sqlx::query("WITH expired AS (SELECT r.id FROM directory_resources r JOIN directory_settings d ON d.connector_id=r.connector_id WHERE r.deleted AND r.payload_ciphertext IS NOT NULL AND r.deleted_at<now()-make_interval(days=>d.retention_days) ORDER BY r.id LIMIT 100 FOR UPDATE OF r SKIP LOCKED) UPDATE directory_resources r SET payload_ciphertext=NULL,payload_key=NULL,payload_nonce=NULL,payload_wrap_nonce=NULL FROM expired e WHERE r.id=e.id")
        .execute(&state.pool).await?;
    sqlx::query("WITH expired AS (SELECT p.id FROM directory_containment_plans p JOIN directory_settings d ON d.connector_id=p.connector_id WHERE p.inventory ? 'users' AND p.created_at<now()-make_interval(days=>d.retention_days) ORDER BY p.id LIMIT 100 FOR UPDATE OF p SKIP LOCKED) UPDATE directory_containment_plans p SET inventory=jsonb_build_object('retired',true,'inventory_sha256',p.inventory_sha256) FROM expired e WHERE p.id=e.id")
        .execute(&state.pool).await?;
    Ok(())
}
