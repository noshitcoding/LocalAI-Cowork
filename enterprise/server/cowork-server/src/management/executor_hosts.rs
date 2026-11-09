//! Immutable execution-host assignments and revocable operator identities.
//! Endpoint addresses, TLS private keys and paths never enter the manager API.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostBinding {
    pub version: u32,
    pub host_id: Uuid,
    pub trust_domain: Uuid,
    pub daemon_id: String,
    pub operator_state_id: Uuid,
}
impl HostBinding {
    fn validate(&self) -> Result<(), ApiError> {
        if self.version != 1
            || self.host_id.is_nil()
            || self.trust_domain.is_nil()
            || self.operator_state_id.is_nil()
            || !(12..=128).contains(&self.daemon_id.len())
            || !self
                .daemon_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":_.-".contains(&b))
        {
            return Err(invalid("invalid executor host binding"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct OperatorPrincipal {
    pub binding: Option<HostBinding>,
    pub enabled: bool,
    credential_id: Option<Uuid>,
}
impl OperatorPrincipal {
    pub fn host_id(&self) -> Option<Uuid> {
        self.binding.as_ref().map(|v| v.host_id)
    }
    pub fn verify_claim(&self, requested: Option<&HostBinding>) -> Result<(), ApiError> {
        if self.binding.as_ref() != requested {
            return Err(denied());
        }
        Ok(())
    }
    pub fn verify_operation(&self, binding: &Value) -> Result<(), ApiError> {
        let expected = self
            .binding
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or(Value::Null);
        if *binding != expected {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn current(&self, tx: &mut Transaction<'_, Postgres>) -> Result<bool, ApiError> {
        let Some(id) = self.credential_id else {
            return Ok(true);
        };
        let enabled:Option<bool>=sqlx::query_scalar("SELECT h.enabled FROM management_executor_host_credentials c JOIN management_executor_hosts h ON h.id=c.host_id WHERE c.id=$1 AND c.host_id=$2 AND c.revoked_at IS NULL")
            .bind(id).bind(self.host_id()).fetch_optional(&mut **tx).await?;
        enabled.ok_or_else(denied)
    }
}

pub(super) async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<OperatorPrincipal, ApiError> {
    let given = headers
        .get("x-cowork-provisioner-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if given.len() > 512 || given.len() < 32 {
        return Err(denied());
    }
    if let Some(operator) = lookup(&state.pool, given).await? {
        return Ok(operator);
    }
    // The old secret is deliberately incapable of acting for any assigned host.
    provisioner_auth(headers)?;
    Ok(OperatorPrincipal {
        binding: None,
        enabled: true,
        credential_id: None,
    })
}

async fn lookup(pool: &sqlx::PgPool, given: &str) -> Result<Option<OperatorPrincipal>, ApiError> {
    let hash = Sha256::digest(given.as_bytes());
    let value: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('credential_id',c.id,'binding',jsonb_build_object('version',1,'host_id',h.id,'trust_domain',h.trust_domain,'daemon_id',h.daemon_id,'operator_state_id',h.operator_state_id),'enabled',h.enabled) FROM management_executor_host_credentials c JOIN management_executor_hosts h ON h.id=c.host_id JOIN management_organisations o ON o.id=h.trust_domain AND o.archived_at IS NULL WHERE c.token_sha256=$1 AND c.revoked_at IS NULL")
        .bind(hash.as_slice()).fetch_optional(pool).await?;
    if let Some(value) = value {
        let binding: HostBinding = serde_json::from_value(value["binding"].clone())?;
        binding.validate()?;
        return Ok(Some(OperatorPrincipal {
            binding: Some(binding),
            enabled: value["enabled"] == true,
            credential_id: Some(serde_json::from_value(value["credential_id"].clone())?),
        }));
    }
    Ok(None)
}

pub(super) async fn instance_host(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    Ok(
        sqlx::query_scalar("SELECT executor_host_id FROM management_instances WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

/// Confidential response-only data: never stored in an operation, audit or DTO.
#[derive(serde::Serialize)]
struct OperatorFleetSecrets {
    version: u32,
    operation_id: Uuid,
    lease_token: Uuid,
    organisation_id: Uuid,
    instance_id: Uuid,
    executor_host_binding: HostBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_epoch: Option<Uuid>,
    fleet_key: String,
}
pub(super) async fn attach_operator_secrets(
    tx: &mut Transaction<'_, Postgres>,
    operator: &OperatorPrincipal,
    operation: &mut Value,
) -> Result<(), ApiError> {
    if operator.binding.is_none() {
        return Ok(());
    }
    operator.verify_operation(&operation["executor_host_binding"])?;
    if !operator.current(tx).await? {
        return Ok(());
    }
    let org: Uuid = serde_json::from_value(operation["organisation_id"].clone())?;
    let instance: Uuid = serde_json::from_value(operation["instance_id"].clone())?;
    let id: Uuid = serde_json::from_value(operation["id"].clone())?;
    let lease: Uuid = serde_json::from_value(operation["lease_token"].clone())?;
    let row=sqlx::query("SELECT i.execution_required,i.execution_epoch FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.id=$1 AND o.instance_id=$2 AND o.organisation_id=$3 AND o.lease_token=$4 AND o.state IN ('running','reconciling') AND (o.state='reconciling' OR o.lease_expires_at>clock_timestamp()) AND o.executor_host_binding=$5 AND i.organisation_id=$3 AND i.executor_host_id=$6 AND o.execution_binding IS NOT DISTINCT FROM $7 FOR SHARE OF o,i")
        .bind(id).bind(instance).bind(org).bind(lease).bind(&operation["executor_host_binding"]).bind(operator.host_id())
        .bind(if operation["execution_binding"].is_null() {None} else {Some(operation["execution_binding"].clone())}).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    if operation["instance"]["id"] != json!(instance)
        || operation["instance"]["organisation_id"] != json!(org)
        || operation["instance"]["executor_host_id"] != json!(operator.host_id())
    {
        return Err(denied());
    }
    let binding: Option<cowork_contracts::enterprise_execution::ExecutionGenerationBinding> =
        serde_json::from_value(operation["execution_binding"].clone())?;
    let epoch = if let Some(binding) = binding {
        if binding.operation_id != id
            || binding.organisation_id != org
            || binding.instance_id != instance
            || binding.source_epoch != row.try_get::<Uuid, _>("execution_epoch")?
            || binding.candidate_epoch.is_nil()
            || binding.source_epoch.is_nil()
        {
            return Err(denied());
        }
        Some(binding.candidate_epoch)
    } else {
        if row.try_get::<bool, _>("execution_required")? {
            return Err(denied());
        }
        None
    };
    if !operator.current(tx).await? {
        return Ok(());
    }
    operation["operator_secrets"] = serde_json::to_value(OperatorFleetSecrets {
        version: if epoch.is_some() { 2 } else { 1 },
        operation_id: id,
        lease_token: lease,
        organisation_id: org,
        instance_id: instance,
        executor_host_binding: operator.binding.clone().ok_or_else(denied)?,
        execution_epoch: epoch,
        fleet_key: if let Some(epoch) = epoch {
            crate::fleet::epoch_key(org, instance, epoch)?
        } else {
            crate::fleet::instance_key(org, instance)?
        },
    })?;
    Ok(())
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capacity {
    cpu_millis: u64,
    memory_mib: u64,
    reserved_cpu_millis: u64,
    reserved_memory_mib: u64,
}
impl Capacity {
    fn validate(&self) -> Result<(), ApiError> {
        if self.cpu_millis == 0
            || self.cpu_millis > 1_024_000
            || self.memory_mib == 0
            || self.memory_mib > 4_194_304
            || self.reserved_cpu_millis >= self.cpu_millis
            || self.reserved_memory_mib >= self.memory_mib
        {
            return Err(invalid("invalid static executor capacity attestation"));
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegisterHost {
    id: Uuid,
    name: String,
    daemon_id: String,
    operator_state_id: Uuid,
    capacity: Option<Capacity>,
    service_plan: Option<host_capacity::ServicePlan>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlHost {
    enabled: bool,
    expected_revision: i64,
}

async fn infrastructure_admin(
    state: &AppState,
    principal: &Principal,
    org: Uuid,
) -> Result<(), ApiError> {
    if !platform(state, principal).await? {
        return Err(denied());
    }
    membership(state, principal, org, true).await?;
    let session = principal.session_id.ok_or_else(denied)?;
    let assured: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>now() AND second_factor_verified_at IS NOT NULL)")
        .bind(session).bind(principal.user_id).fetch_one(&state.pool).await?;
    if !assured {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    membership(&state, &principal, org, true).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(h)-'created_by' FROM management_executor_hosts h WHERE trust_domain=$1 ORDER BY created_at,id").bind(org).fetch_all(&state.pool).await?))
}

pub(super) async fn instances(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, host)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_executor_hosts WHERE id=$1 AND trust_domain=$2)",
    )
    .bind(host)
    .bind(org)
    .fetch_one(&state.pool)
    .await?;
    if !exists {
        return Err(denied());
    }
    Ok(Json(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'state',state,'backend',backend,'revision',revision,'executor_host_id',executor_host_id) FROM management_instances WHERE organisation_id=$1 AND executor_host_id=$2 AND state<>'deleted' ORDER BY created_at,id")
        .bind(org).bind(host).fetch_all(&state.pool).await?))
}
pub(super) async fn register(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<RegisterHost>,
) -> Result<Json<Value>, ApiError> {
    infrastructure_admin(&state, &principal, org).await?;
    HostBinding {
        version: 1,
        host_id: request.id,
        trust_domain: org,
        daemon_id: request.daemon_id.clone(),
        operator_state_id: request.operator_state_id,
    }
    .validate()?;
    if let Some(capacity) = &request.capacity {
        capacity.validate()?;
    }
    if let Some(plan) = &request.service_plan {
        plan.validate()?;
    }
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    sqlx::query("INSERT INTO management_executor_hosts(id,trust_domain,name,daemon_id,operator_state_id,capacity,created_by,service_plan) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(request.id).bind(org).bind(name(&request.name)?).bind(&request.daemon_id).bind(request.operator_state_id)
        .bind(request.capacity.as_ref().map(serde_json::to_value).transpose()?).bind(principal.user_id)
        .bind(request.service_plan.as_ref().map(serde_json::to_value).transpose()?).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "executor_host.register",
        request.id,
        json!({"operator_state_id":request.operator_state_id,"capacity":request.capacity,"service_plan":request.service_plan}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":request.id,"trust_domain":org,"revision":1}),
    ))
}
pub(super) async fn control(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, host)): Path<(Uuid, Uuid)>,
    Json(request): Json<ControlHost>,
) -> Result<Json<Value>, ApiError> {
    infrastructure_admin(&state, &principal, org).await?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    let changed=sqlx::query("UPDATE management_executor_hosts SET enabled=$3,revision=revision+1 WHERE id=$1 AND trust_domain=$2 AND revision=$4")
        .bind(host).bind(org).bind(request.enabled).bind(request.expected_revision).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "executor host changed or is inaccessible".into(),
        ));
    }
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "executor_host.control",
        host,
        json!({"enabled":request.enabled}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":host,"enabled":request.enabled,"revision":request.expected_revision+1}),
    ))
}
pub(super) async fn issue_credential(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, host)): Path<(Uuid, Uuid)>,
) -> Result<
    (
        [(axum::http::header::HeaderName, &'static str); 1],
        Json<Value>,
    ),
    ApiError,
> {
    infrastructure_admin(&state, &principal, org).await?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_executor_hosts WHERE id=$1 AND trust_domain=$2 AND enabled)").bind(host).bind(org).fetch_one(&mut *tx).await?;
    if !exists {
        return Err(denied());
    }
    let id = Uuid::new_v4();
    let token = format!(
        "cowork-host.{}.{}{}",
        id.simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    sqlx::query("INSERT INTO management_executor_host_credentials(id,host_id,token_sha256,created_by) VALUES($1,$2,$3,$4)")
        .bind(id).bind(host).bind(Sha256::digest(token.as_bytes()).as_slice()).bind(principal.user_id).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "executor_host.credential.issue",
        host,
        json!({"credential_id":id}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({"id":id,"host_id":host,"token":token})),
    ))
}
pub(super) async fn revoke_credential(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, host, id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    infrastructure_admin(&state, &principal, org).await?;
    let mut tx = state.pool.begin().await?;
    rollouts::lock(&mut tx).await?;
    let changed=sqlx::query("UPDATE management_executor_host_credentials c SET revoked_at=coalesce(revoked_at,now()) FROM management_executor_hosts h WHERE c.id=$1 AND c.host_id=$2 AND h.id=c.host_id AND h.trust_domain=$3")
        .bind(id).bind(host).bind(org).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(denied());
    }
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "executor_host.credential.revoke",
        host,
        json!({"credential_id":id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"revoked":true})))
}
pub(super) async fn create_instance(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, host)): Path<(Uuid, Uuid)>,
    Json(request): Json<CreateInstance>,
) -> Result<Json<Value>, ApiError> {
    infrastructure_admin(&state, &principal, org).await?;
    super::create_instance_with_host(state, principal, org, request, Some(host)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> HostBinding {
        HostBinding {
            version: 1,
            host_id: Uuid::new_v4(),
            trust_domain: Uuid::new_v4(),
            daemon_id: "a".repeat(64),
            operator_state_id: Uuid::new_v4(),
        }
    }
    #[test]
    fn legacy_operator_cannot_claim_or_complete_scoped_work() {
        let legacy = OperatorPrincipal {
            binding: None,
            enabled: true,
            credential_id: None,
        };
        let host = binding();
        assert!(legacy.verify_claim(Some(&host)).is_err());
        assert!(legacy.verify_operation(&json!(host)).is_err());
        assert!(legacy.verify_claim(None).is_ok());
        assert!(legacy.verify_operation(&Value::Null).is_ok());
    }
    #[test]
    fn scoped_operator_requires_every_frozen_identity_field() {
        let host = binding();
        let principal = OperatorPrincipal {
            binding: Some(host.clone()),
            enabled: true,
            credential_id: Some(Uuid::new_v4()),
        };
        assert!(principal.verify_claim(None).is_err());
        assert!(principal.verify_operation(&json!(host)).is_ok());
        for changed in [
            HostBinding {
                daemon_id: "b".repeat(64),
                ..host.clone()
            },
            HostBinding {
                trust_domain: Uuid::new_v4(),
                ..host.clone()
            },
            HostBinding {
                operator_state_id: Uuid::new_v4(),
                ..host.clone()
            },
            HostBinding {
                host_id: Uuid::new_v4(),
                ..host.clone()
            },
        ] {
            assert!(principal.verify_claim(Some(&changed)).is_err());
            assert!(principal.verify_operation(&json!(changed)).is_err());
        }
    }

    #[test]
    fn host_identity_and_capacity_inputs_are_bounded_and_closed() {
        let valid = binding();
        assert!(valid.validate().is_ok());
        for changed in [
            HostBinding {
                version: 2,
                ..valid.clone()
            },
            HostBinding {
                operator_state_id: Uuid::nil(),
                ..valid.clone()
            },
            HostBinding {
                daemon_id: "wrong/daemon".into(),
                ..valid.clone()
            },
        ] {
            assert!(changed.validate().is_err());
        }
        let mut unknown = json!(valid);
        unknown["endpoint"] = json!("tcp://untrusted:2376");
        assert!(serde_json::from_value::<HostBinding>(unknown).is_err());
        for capacity in [
            Capacity {
                cpu_millis: 0,
                memory_mib: 4096,
                reserved_cpu_millis: 0,
                reserved_memory_mib: 512,
            },
            Capacity {
                cpu_millis: 4000,
                memory_mib: 4096,
                reserved_cpu_millis: 4000,
                reserved_memory_mib: 512,
            },
            Capacity {
                cpu_millis: 4000,
                memory_mib: 4096,
                reserved_cpu_millis: 1000,
                reserved_memory_mib: 4096,
            },
        ] {
            assert!(capacity.validate().is_err());
        }
    }

    // All tables and functions are session-local. Even the migration under test
    // cannot alter application rows or persistent functions in the supplied DB.
    async fn host_test_pool() -> sqlx::PgPool {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE users(id UUID PRIMARY KEY);
            CREATE TEMP TABLE management_organisations(id UUID PRIMARY KEY,archived_at TIMESTAMPTZ);
            CREATE TEMP TABLE management_instances(id UUID PRIMARY KEY,organisation_id UUID NOT NULL,backend TEXT NOT NULL DEFAULT 'compose',state TEXT NOT NULL DEFAULT 'stopped',execution_required BOOLEAN NOT NULL DEFAULT false,execution_epoch UUID NOT NULL DEFAULT gen_random_uuid());
            CREATE TEMP TABLE management_operations(id UUID PRIMARY KEY,instance_id UUID NOT NULL,organisation_id UUID NOT NULL,state TEXT NOT NULL DEFAULT 'succeeded',lease_token UUID,lease_expires_at TIMESTAMPTZ,execution_binding JSONB,backend_profile JSONB);
            CREATE TEMP TABLE management_releases(id TEXT PRIMARY KEY,manifest JSONB NOT NULL,active BOOLEAN NOT NULL DEFAULT TRUE,refreshed_at TIMESTAMPTZ NOT NULL DEFAULT now());
            CREATE TEMP TABLE management_release_qualifications(release_id TEXT NOT NULL REFERENCES management_releases(id),source_release TEXT NOT NULL,operation_id UUID NOT NULL REFERENCES management_operations(id),created_at TIMESTAMPTZ NOT NULL DEFAULT now(),PRIMARY KEY(release_id,source_release));
        "#).execute(&mut *connection).await.unwrap();
        let mut migration = include_str!("../../migrations/0053_executor_host_assignments.sql")
            .replace("CREATE TABLE", "CREATE TEMP TABLE");
        for function in [
            "preserve_executor_host_credential",
            "preserve_executor_host_identity",
            "preserve_instance_executor_host",
            "freeze_operation_executor_host",
            "validate_executor_host_qualification",
        ] {
            migration =
                migration.replace(&format!("{function}()"), &format!("pg_temp.{function}()"));
        }
        sqlx::raw_sql(&migration)
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        pool
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_host_assignments_tokens_and_canaries_are_scoped() {
        let pool = host_test_pool().await;
        let first = binding();
        let second = HostBinding {
            host_id: Uuid::new_v4(),
            daemon_id: "b".repeat(64),
            operator_state_id: Uuid::new_v4(),
            ..first.clone()
        };
        let other_org = Uuid::new_v4();
        let user = Uuid::new_v4();
        sqlx::query("INSERT INTO users VALUES($1)")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        for org in [first.trust_domain, other_org] {
            sqlx::query("INSERT INTO management_organisations(id) VALUES($1)")
                .bind(org)
                .execute(&pool)
                .await
                .unwrap();
        }
        for host in [&first, &second] {
            sqlx::query("INSERT INTO management_executor_hosts(id,trust_domain,name,daemon_id,operator_state_id,created_by) VALUES($1,$2,'fixture',$3,$4,$5)")
                .bind(host.host_id).bind(host.trust_domain).bind(&host.daemon_id).bind(host.operator_state_id).bind(user).execute(&pool).await.unwrap();
        }
        let instances = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let operations = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        for (index, host) in [Some(first.host_id), Some(second.host_id), None]
            .into_iter()
            .enumerate()
        {
            sqlx::query("INSERT INTO management_instances(id,organisation_id,executor_host_id) VALUES($1,$2,$3)").bind(instances[index]).bind(first.trust_domain).bind(host).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO management_operations(id,instance_id,organisation_id) VALUES($1,$2,$3)").bind(operations[index]).bind(instances[index]).bind(first.trust_domain).execute(&pool).await.unwrap();
        }
        let actual: Value = sqlx::query_scalar(
            "SELECT executor_host_binding FROM management_operations WHERE id=$1",
        )
        .bind(operations[0])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(actual, json!(first));
        assert!(sqlx::query("INSERT INTO management_instances(id,organisation_id,executor_host_id) VALUES($1,$2,$3)").bind(Uuid::new_v4()).bind(other_org).bind(first.host_id).execute(&pool).await.is_err());
        assert!(
            sqlx::query("UPDATE management_instances SET executor_host_id=$2 WHERE id=$1")
                .bind(instances[0])
                .bind(second.host_id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE management_instances SET executor_host_id=$2 WHERE id=$1")
                .bind(instances[2])
                .bind(first.host_id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(sqlx::query(
            "UPDATE management_executor_hosts SET daemon_id='different-daemon' WHERE id=$1"
        )
        .bind(first.host_id)
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE management_operations SET executor_host_binding=$2 WHERE id=$1"
        )
        .bind(operations[0])
        .bind(json!(second))
        .execute(&pool)
        .await
        .is_err());
        let credential = Uuid::new_v4();
        let token = "test-only-scoped-operator-token-not-a-live-secret";
        sqlx::query("INSERT INTO management_executor_host_credentials(id,host_id,token_sha256,created_by) VALUES($1,$2,$3,$4)").bind(credential).bind(first.host_id).bind(Sha256::digest(token.as_bytes()).as_slice()).bind(user).execute(&pool).await.unwrap();
        let operator = lookup(&pool, token).await.unwrap().unwrap();
        assert_eq!(operator.binding, Some(first.clone()));
        assert!(operator.verify_operation(&json!(second)).is_err());
        assert!(operator.verify_operation(&Value::Null).is_err());
        struct FixtureKey {
            path: std::path::PathBuf,
            previous: Option<std::ffi::OsString>,
        }
        impl Drop for FixtureKey {
            fn drop(&mut self) {
                match &self.previous {
                    Some(previous) => std::env::set_var("COWORK_PROVISIONER_TOKEN_FILE", previous),
                    None => std::env::remove_var("COWORK_PROVISIONER_TOKEN_FILE"),
                }
                let _ = std::fs::remove_file(&self.path);
            }
        }
        let key_path =
            std::env::temp_dir().join(format!("cowork-host-key-test-{}", Uuid::new_v4()));
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&key_path)
            .unwrap();
        let _key_guard = FixtureKey {
            path: key_path.clone(),
            previous: std::env::var_os("COWORK_PROVISIONER_TOKEN_FILE"),
        };
        file.write_all(b"fixture-only-manager-global-key-0000000000000000")
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::env::set_var("COWORK_PROVISIONER_TOKEN_FILE", &key_path);
        let lease = Uuid::new_v4();
        sqlx::query("UPDATE management_operations SET lease_token=$2,state='running',lease_expires_at=clock_timestamp()+interval '1 hour' WHERE id=$1")
            .bind(operations[0])
            .bind(lease)
            .execute(&pool)
            .await
            .unwrap();
        let operation:Value=sqlx::query_scalar("SELECT to_jsonb(o)||jsonb_build_object('instance',to_jsonb(i)) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.id=$1").bind(operations[0]).fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let mut private = operation.clone();
        attach_operator_secrets(&mut tx, &operator, &mut private)
            .await
            .unwrap();
        assert_eq!(
            private["operator_secrets"]["fleet_key"],
            json!(crate::fleet::instance_key(first.trust_domain, instances[0]).unwrap())
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cowork-instance-key",
            private["operator_secrets"]["fleet_key"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        );
        assert!(crate::fleet::verify_instance_key(
            &mut tx,
            &headers,
            first.trust_domain,
            instances[0]
        )
        .await
        .is_ok());
        assert!(crate::fleet::verify_instance_key(
            &mut tx,
            &headers,
            first.trust_domain,
            instances[1]
        )
        .await
        .is_err());
        for changed in [
            {
                let mut value = operation.clone();
                value["instance"]["executor_host_id"] = json!(second.host_id);
                value
            },
            {
                let mut value = operation.clone();
                value["instance_id"] = json!(instances[1]);
                value
            },
            {
                let mut value = operation.clone();
                value["lease_token"] = json!(Uuid::new_v4());
                value
            },
        ] {
            let mut changed = changed;
            assert!(attach_operator_secrets(&mut tx, &operator, &mut changed)
                .await
                .is_err());
            assert!(changed["operator_secrets"].is_null());
        }
        let stored: Value =
            sqlx::query_scalar("SELECT to_jsonb(o) FROM management_operations o WHERE id=$1")
                .bind(operations[0])
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert!(stored["operator_secrets"].is_null());
        let source_epoch = Uuid::new_v4();
        let candidate_epoch = Uuid::new_v4();
        let generation = json!({"operation_id":operations[0],"organisation_id":first.trust_domain,
            "instance_id":instances[0],"source_epoch":source_epoch,"candidate_epoch":candidate_epoch});
        sqlx::query("UPDATE management_instances SET execution_required=TRUE,execution_epoch=$2 WHERE id=$1")
            .bind(instances[0]).bind(source_epoch).execute(&mut *tx).await.unwrap();
        sqlx::query("UPDATE management_operations SET execution_binding=$2 WHERE id=$1")
            .bind(operations[0])
            .bind(&generation)
            .execute(&mut *tx)
            .await
            .unwrap();
        let enrolled: Value = sqlx::query_scalar("SELECT to_jsonb(o)||jsonb_build_object('instance',to_jsonb(i)) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE o.id=$1")
            .bind(operations[0]).fetch_one(&mut *tx).await.unwrap();
        let mut candidate = enrolled.clone();
        attach_operator_secrets(&mut tx, &operator, &mut candidate)
            .await
            .unwrap();
        assert_eq!(candidate["operator_secrets"]["version"], json!(2));
        assert_eq!(
            candidate["operator_secrets"]["execution_epoch"],
            json!(candidate_epoch)
        );
        assert_eq!(
            candidate["operator_secrets"]["fleet_key"],
            json!(
                crate::fleet::epoch_key(first.trust_domain, instances[0], candidate_epoch).unwrap()
            )
        );
        assert!(crate::fleet::verify_instance_key(
            &mut tx,
            &headers,
            first.trust_domain,
            instances[0]
        )
        .await
        .is_err());
        for field in ["candidate_epoch", "organisation_id", "operation_id"] {
            let mut changed = enrolled.clone();
            changed["execution_binding"][field] = json!(Uuid::new_v4());
            assert!(attach_operator_secrets(&mut tx, &operator, &mut changed)
                .await
                .is_err());
            assert!(changed["operator_secrets"].is_null());
        }
        let mut missing = enrolled.clone();
        missing["execution_binding"] = Value::Null;
        assert!(attach_operator_secrets(&mut tx, &operator, &mut missing)
            .await
            .is_err());
        sqlx::query("UPDATE management_instances SET execution_epoch=$2 WHERE id=$1")
            .bind(instances[0])
            .bind(Uuid::new_v4())
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            attach_operator_secrets(&mut tx, &operator, &mut enrolled.clone())
                .await
                .is_err()
        );
        sqlx::query("UPDATE management_instances SET execution_epoch=$2 WHERE id=$1")
            .bind(instances[0])
            .bind(source_epoch)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE management_operations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
            .bind(operations[0]).execute(&mut *tx).await.unwrap();
        assert!(
            attach_operator_secrets(&mut tx, &operator, &mut enrolled.clone())
                .await
                .is_err()
        );
        sqlx::query("UPDATE management_operations SET state='reconciling' WHERE id=$1")
            .bind(operations[0])
            .execute(&mut *tx)
            .await
            .unwrap();
        attach_operator_secrets(&mut tx, &operator, &mut enrolled.clone())
            .await
            .unwrap();
        println!("operator-fleet-check: exact candidate epoch v2; legacy, forged binding, stale source epoch and expired running lease denied; reconciliation keeps original lease");
        tx.commit().await.unwrap();
        sqlx::query("UPDATE management_operations SET state='succeeded' WHERE id=$1")
            .bind(operations[0])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE management_executor_hosts SET enabled=FALSE WHERE id=$1")
            .bind(first.host_id)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(!operator.current(&mut tx).await.unwrap());
        tx.commit().await.unwrap();
        sqlx::query("UPDATE management_executor_hosts SET enabled=TRUE WHERE id=$1")
            .bind(first.host_id)
            .execute(&pool)
            .await
            .unwrap();
        let release = "a".repeat(64);
        let source = "b".repeat(64);
        sqlx::query("INSERT INTO management_releases(id,manifest) VALUES($1,$2)")
            .bind(&release)
            .bind(json!({"supported_sources":[source]}))
            .execute(&pool)
            .await
            .unwrap();
        for host in [first.host_id, second.host_id] {
            sqlx::query(
                "INSERT INTO management_executor_host_releases(host_id,release_id) VALUES($1,$2)",
            )
            .bind(host)
            .bind(&release)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO management_release_qualifications(release_id,source_release,operation_id,executor_host_id) VALUES($1,$2,$3,$4)").bind(&release).bind(&source).bind(operations[0]).bind(first.host_id).execute(&pool).await.unwrap();
        assert!(sqlx::query("INSERT INTO management_release_qualifications(release_id,source_release,operation_id,executor_host_id) VALUES($1,$2,$3,$4)").bind(&release).bind(&source).bind(operations[0]).bind(second.host_id).execute(&pool).await.is_err());
        let mut tx = pool.begin().await.unwrap();
        assert!(releases::selection_allowed(
            &mut tx,
            &release,
            &source,
            false,
            Some(first.host_id)
        )
        .await
        .unwrap());
        assert!(!releases::selection_allowed(
            &mut tx,
            &release,
            &source,
            false,
            Some(second.host_id)
        )
        .await
        .unwrap());
        assert!(
            !releases::selection_allowed(&mut tx, &release, &source, false, None)
                .await
                .unwrap()
        );
        assert!(releases::selection_allowed(
            &mut tx,
            &release,
            &source,
            true,
            Some(second.host_id)
        )
        .await
        .unwrap());
        sqlx::query("UPDATE management_executor_host_releases SET active=FALSE WHERE host_id=$1")
            .bind(first.host_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(!releases::selection_allowed(
            &mut tx,
            &release,
            &source,
            true,
            Some(first.host_id)
        )
        .await
        .unwrap());
        assert!(releases::selection_allowed(
            &mut tx,
            &release,
            &source,
            true,
            Some(second.host_id)
        )
        .await
        .unwrap());
        tx.commit().await.unwrap();
        sqlx::query("UPDATE management_executor_host_credentials SET revoked_at=now() WHERE id=$1")
            .bind(credential)
            .execute(&pool)
            .await
            .unwrap();
        assert!(lookup(&pool, token).await.unwrap().is_none());
        assert!(sqlx::query(
            "UPDATE management_executor_host_credentials SET revoked_at=NULL WHERE id=$1"
        )
        .bind(credential)
        .execute(&pool)
        .await
        .is_err());
        let mut tx = pool.begin().await.unwrap();
        assert!(operator.current(&mut tx).await.is_err());
    }
}
