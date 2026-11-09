//! Operator configuration and private transaction intents for personal model IO.
//! RunSpec, Change and event JSON never select a monetary transition.
use super::*;
pub use cowork_model_accounting::money::*;
use cowork_model_accounting::persistence::{
    self, AccountingOperation, AccountingOperationKind, LockedLedger,
};
pub use cowork_model_accounting::received::{
    CompletedModelReceipt, InterruptedModelReceipt, ModelDispatchScope,
};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonetaryTokenMode {
    /// Reserve the complete frozen context limit, without tokenizer parity.
    ConservativeContextUpperBound,
    /// Independently count the selected immutable operator-qualified tokenizer
    /// and complete template against the same prepared request being admitted.
    ExactArtifactRenderer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorModelDeployment {
    pub id: String,
    #[serde(deserialize_with = "strict_provider_profile")]
    pub provider: ProviderProfile,
    pub model_sha256: String,
    pub price_id: String,
    pub price_version: u64,
    pub token_mode: MonetaryTokenMode,
}

fn strict_provider_profile<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ProviderProfile, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Profile {
        id: String,
        name: String,
        kind: ProviderKind,
        endpoint: String,
        model: String,
        revision: i64,
        context_tokens: u32,
        credential_required: bool,
        credential_ready: bool,
        #[serde(default)]
        capabilities: ModelCapabilities,
    }
    let value = Profile::deserialize(deserializer)?;
    Ok(ProviderProfile {
        generation: Default::default(),
        id: value.id,
        name: value.name,
        kind: value.kind,
        endpoint: value.endpoint,
        model: value.model,
        revision: value.revision,
        context_tokens: value.context_tokens,
        credential_required: value.credential_required,
        credential_ready: value.credential_ready,
        capabilities: value.capabilities,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalMoneyDeployment {
    pub deployment: OperatorModelDeployment,
    pub price: PriceVersion,
}

/// Private installation operator configuration, never a public run/tool claim.
/// Activation is durable and immutable; absence of a file cannot disable it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalMoneyConfiguration {
    pub format_version: u32,
    pub installation: PoolPolicy,
    pub project_limit_units: u64,
    pub run_limit_units: u64,
    pub pool_policy_version: u64,
    pub deployments: Vec<PersonalMoneyDeployment>,
}

impl PersonalMoneyConfiguration {
    pub(super) fn validate(&self) -> Result<()> {
        if self.format_version != 1
            || self.pool_policy_version == 0
            || self.installation.version == 0
            || !matches!(&self.installation.scope, PoolScope::Installation(id) if !id.trim().is_empty() && id.len() <= 128)
            || self.deployments.is_empty()
            || self.deployments.len() > 128
        {
            return Err(Error::Validation(
                "invalid_personal_money_configuration".into(),
            ));
        }
        self.installation
            .currency
            .validate()
            .map_err(accounting_error)?;
        let mut profiles = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        for entry in &self.deployments {
            let deployment = &entry.deployment;
            let profile = &deployment.provider;
            valid_text(&deployment.id, 128)?;
            valid_text(&profile.id, 128)?;
            valid_text(&profile.name, 100)?;
            valid_text(&profile.model, 200)?;
            if deployment.id.trim().is_empty()
                || profile.id.trim().is_empty()
                || profile.name.trim().is_empty()
                || profile.model.trim().is_empty()
                || profile.revision <= 0
                || !(1024..=2_000_000).contains(&profile.context_tokens)
                || matches!(profile.kind, ProviderKind::Test)
                || !ids.insert(deployment.id.clone())
                || !profiles.insert(canonical_hash(profile))
                || deployment.model_sha256 != entry.price.model_sha256
                || deployment.price_id != entry.price.id
                || deployment.price_version != entry.price.version
                || profile.id != entry.price.provider_id
                || entry.price.currency != self.installation.currency
            {
                return Err(Error::Forbidden);
            }
            let url = reqwest::Url::parse(&profile.endpoint)
                .map_err(|_| Error::Validation("invalid_personal_money_provider_url".into()))?;
            let local = matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            );
            if !(url.scheme() == "https" || url.scheme() == "http" && local)
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::Forbidden);
            }
            entry.price.validate().map_err(accounting_error)?;
            entry
                .price
                .reserve(
                    u64::from(profile.context_tokens),
                    u64::from(profile.context_tokens),
                )
                .map_err(accounting_error)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPersonalRunBinding {
    run_id: String,
    project_id: String,
    installation_id: String,
    deployment_id: String,
    run_spec_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderDeploymentBinding {
    #[serde(deserialize_with = "strict_provider_profile")]
    provider: ProviderProfile,
    deployment_id: String,
    deployment_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredPersonalRunBinding {
    format_version: u32,
    run_id: String,
    project_id: String,
    installation_id: String,
    run_spec_sha256: String,
    configuration_sha256: String,
    provider_deployments: Vec<ProviderDeploymentBinding>,
}

/// Serialize historical bindings byte-for-value identically: no new default
/// field or tag changes their immutable SHA or original dispatch receipts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum PersonalRunBinding {
    Legacy(LegacyPersonalRunBinding),
    Configured(ConfiguredPersonalRunBinding),
}

impl PersonalRunBinding {
    fn run_id(&self) -> &str {
        match self {
            Self::Legacy(v) => &v.run_id,
            Self::Configured(v) => &v.run_id,
        }
    }
    fn project_id(&self) -> &str {
        match self {
            Self::Legacy(v) => &v.project_id,
            Self::Configured(v) => &v.project_id,
        }
    }
    fn installation_id(&self) -> &str {
        match self {
            Self::Legacy(v) => &v.installation_id,
            Self::Configured(v) => &v.installation_id,
        }
    }
    fn run_spec_sha256(&self) -> &str {
        match self {
            Self::Legacy(v) => &v.run_spec_sha256,
            Self::Configured(v) => &v.run_spec_sha256,
        }
    }
}

/// This private fence is minted before write-key/order/registry locks, in the
/// same transaction which will create and enroll the actual run.
pub(crate) struct CreationMoneyFence(());
pub(crate) async fn lock_creation(tx: &mut Transaction<'_, Any>) -> Result<CreationMoneyFence> {
    persistence::lock_global(tx)
        .await
        .map_err(accounting_error)?;
    Ok(CreationMoneyFence(()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptBinding {
    id: String,
    run_id: String,
    attempt_id: String,
    logical_sha256: String,
    wire_sha256: String,
    provider_profile_sha256: String,
    model_spec_sha256: String,
    deployment_sha256: String,
    admission_receipt_sha256: String,
    lease_fence_sha256: String,
}

pub(crate) enum AccountingIntent<'a> {
    Admit {
        attempt_id: &'a str,
        logical: &'a str,
        wire: &'a str,
    },
    Settle {
        attempt_id: &'a str,
        receipt: &'a CompletedModelReceipt,
    },
}

pub(crate) fn accounting_error(error: cowork_model_accounting::Error) -> Error {
    match error {
        cowork_model_accounting::Error::BudgetExceeded => Error::BudgetExceeded,
        cowork_model_accounting::Error::Conflict
        | cowork_model_accounting::Error::StaleRevision => Error::Conflict,
        other => Error::Storage(Box::new(other)),
    }
}

pub(crate) async fn migrate(tx: &mut Transaction<'_, Any>) -> Result<()> {
    for sql in [
        "CREATE TABLE IF NOT EXISTS accounting_personal_installation (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS accounting_provider_deployments (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS accounting_personal_runs (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS accounting_model_bindings (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)",
    ] {
        sqlx::query(sql)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

pub(crate) async fn accounted(tx: &mut Transaction<'_, Any>, run_id: &str) -> Result<bool> {
    Ok(
        read::<PersonalRunBinding>(tx, "accounting_personal_runs", run_id)
            .await?
            .is_some(),
    )
}

async fn read<T: serde::de::DeserializeOwned>(
    tx: &mut Transaction<'_, Any>,
    table: &str,
    id: &str,
) -> Result<Option<T>> {
    let query = format!("SELECT payload_sha256,payload FROM {table} WHERE id=$1");
    let Some(row) = sqlx::query(&query)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
    else {
        return Ok(None);
    };
    let payload: String = row.try_get("payload").map_err(storage_error)?;
    if row
        .try_get::<String, _>("payload_sha256")
        .map_err(storage_error)?
        != hash(payload.as_bytes())
    {
        return Err(Error::Forbidden);
    }
    Ok(Some(parse(&payload)?))
}

async fn immutable<T: Serialize + serde::de::DeserializeOwned>(
    tx: &mut Transaction<'_, Any>,
    table: &str,
    id: &str,
    value: &T,
) -> Result<()> {
    if let Some(old) = read::<T>(tx, table, id).await? {
        return if canonical_hash(&old) == canonical_hash(value) {
            Ok(())
        } else {
            Err(Error::Conflict)
        };
    }
    let payload = json_string(value)?;
    sqlx::query(&format!(
        "INSERT INTO {table}(id,payload_sha256,payload) VALUES($1,$2,$3)"
    ))
    .bind(id)
    .bind(hash(payload.as_bytes()))
    .bind(payload)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}

/// A globally unique ledger identity even when workflow turn IDs are run-local.
pub fn monetary_attempt_id(run: &str, attempt: &str) -> String {
    format!(
        "model:{}",
        canonical_hash(&json!({"format":1,"run_id":run,"attempt_id":attempt}))
    )
}

fn dispatch_scope(context: &AttemptBinding) -> Result<ModelDispatchScope> {
    ModelDispatchScope::from_committed_admission(
        &context.run_id,
        &context.attempt_id,
        &context.id,
        &context.model_spec_sha256,
        &context.provider_profile_sha256,
        &context.logical_sha256,
        &context.wire_sha256,
        &context.admission_receipt_sha256,
        &context.lease_fence_sha256,
    )
    .map_err(accounting_error)
}

async fn selected_deployment(
    tx: &mut Transaction<'_, Any>,
    binding: &PersonalRunBinding,
    profile: &ProviderProfile,
) -> Result<OperatorModelDeployment> {
    let (id, expected_hash) = match binding {
        PersonalRunBinding::Legacy(legacy) => (legacy.deployment_id.as_str(), None),
        PersonalRunBinding::Configured(configured) => {
            let hashes: Vec<_> = configured
                .provider_deployments
                .iter()
                .map(|v| canonical_hash(&v.provider))
                .collect();
            if configured.format_version != 2
                || configured.configuration_sha256.len() != 64
                || !configured
                    .configuration_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
                || hashes.is_empty()
                || hashes.len() > 128
                || hashes.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(Error::Forbidden);
            }
            let matches: Vec<_> = configured
                .provider_deployments
                .iter()
                .filter(|v| canonical_hash(&v.provider) == canonical_hash(profile))
                .collect();
            if matches.len() != 1 {
                return Err(Error::Forbidden);
            }
            let selected = matches[0];
            (
                selected.deployment_id.as_str(),
                Some(selected.deployment_sha256.as_str()),
            )
        }
    };
    let deployment: OperatorModelDeployment = read(tx, "accounting_provider_deployments", id)
        .await?
        .ok_or(Error::Forbidden)?;
    if deployment.id != id
        || canonical_hash(&deployment.provider) != canonical_hash(profile)
        || expected_hash.is_some_and(|expected| expected != canonical_hash(&deployment))
    {
        return Err(Error::Forbidden);
    }
    Ok(deployment)
}

/// Runs only inside the actual insertion transaction and after full Workflow
/// compilation. No caller-selected scope, price or deployment is accepted.
pub(crate) async fn enroll_created_run(
    tx: &mut Transaction<'_, Any>,
    _fence: &CreationMoneyFence,
    record: &RunRecord,
    workflow: Option<&cowork_domain::workflows::WorkflowPlan>,
) -> Result<()> {
    let Some(config) = read::<PersonalMoneyConfiguration>(
        tx,
        "accounting_personal_configuration",
        "personal-money-configuration-v1",
    )
    .await?
    else {
        return Ok(());
    };
    config.validate()?;
    let tenant: Option<String> = sqlx::query_scalar("SELECT id FROM tenant_projects WHERE id=$1")
        .bind(&record.spec.project_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    if tenant.is_some() {
        return Err(Error::Forbidden);
    }
    // Native file restorations cannot dispatch models and need no model pool.
    if record.spec.file_action.is_some() {
        return Ok(());
    }
    let mut profiles: Vec<_> = match workflow {
        Some(plan) => plan
            .roles
            .values()
            .map(|role| role.provider.clone())
            .collect(),
        None => vec![record.spec.provider.clone().ok_or(Error::Forbidden)?],
    };
    profiles.sort_by_key(canonical_hash);
    profiles.dedup_by(|a, b| canonical_hash(a) == canonical_hash(b));
    if profiles.is_empty() {
        return Err(Error::Forbidden);
    }
    let mut locked = persistence::load_locked(tx)
        .await
        .map_err(accounting_error)?;
    let mut provider_deployments = Vec::new();
    for profile in profiles {
        let matches: Vec<_> = config
            .deployments
            .iter()
            .filter(|entry| canonical_hash(&entry.deployment.provider) == canonical_hash(&profile))
            .collect();
        if matches.len() != 1 {
            return Err(Error::Forbidden);
        }
        let deployment = &matches[0].deployment;
        let persisted: OperatorModelDeployment =
            read(tx, "accounting_provider_deployments", &deployment.id)
                .await?
                .ok_or(Error::Forbidden)?;
        if canonical_hash(&persisted) != canonical_hash(deployment) {
            return Err(Error::Forbidden);
        }
        provider_deployments.push(ProviderDeploymentBinding {
            provider: profile,
            deployment_id: deployment.id.clone(),
            deployment_sha256: canonical_hash(deployment),
        });
    }
    let PoolScope::Installation(installation_id) = &config.installation.scope else {
        return Err(Error::Forbidden);
    };
    let binding = PersonalRunBinding::Configured(ConfiguredPersonalRunBinding {
        format_version: 2,
        run_id: record.id.clone(),
        project_id: record.spec.project_id.clone(),
        installation_id: installation_id.clone(),
        run_spec_sha256: canonical_hash(&record.spec),
        configuration_sha256: canonical_hash(&config),
        provider_deployments,
    });
    for (scope, limit_units) in [
        (
            PoolScope::Project(record.spec.project_id.clone()),
            config.project_limit_units,
        ),
        (PoolScope::Run(record.id.clone()), config.run_limit_units),
    ] {
        locked
            .ledger
            .activate_pool(
                locked.ledger.revision(),
                PoolPolicy {
                    scope,
                    version: config.pool_policy_version,
                    currency: config.installation.currency.clone(),
                    limit_units,
                },
            )
            .map_err(accounting_error)?;
    }
    immutable(tx, "accounting_personal_runs", &record.id, &binding).await?;
    locked.persist(tx).await.map_err(accounting_error)?;
    Ok(())
}

impl Store {
    pub async fn configure_personal_money(
        &self,
        config: &PersonalMoneyConfiguration,
    ) -> Result<()> {
        config.validate()?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = persistence::load_locked(&mut tx)
            .await
            .map_err(accounting_error)?;
        if let Some(old) = read::<PersonalMoneyConfiguration>(
            &mut tx,
            "accounting_personal_configuration",
            "personal-money-configuration-v1",
        )
        .await?
        {
            if canonical_hash(&old) != canonical_hash(config) {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        immutable(
            &mut tx,
            "accounting_personal_installation",
            "personal-installation-v1",
            &config.installation,
        )
        .await?;
        for entry in &config.deployments {
            immutable(
                &mut tx,
                "accounting_provider_deployments",
                &entry.deployment.id,
                &entry.deployment,
            )
            .await?;
            locked
                .ledger
                .register_price(locked.ledger.revision(), entry.price.clone())
                .map_err(accounting_error)?;
        }
        locked
            .ledger
            .activate_pool(locked.ledger.revision(), config.installation.clone())
            .map_err(accounting_error)?;
        locked.persist(&mut tx).await.map_err(accounting_error)?;
        immutable(
            &mut tx,
            "accounting_personal_configuration",
            "personal-money-configuration-v1",
            config,
        )
        .await?;
        tx.commit().await.map_err(storage_error)
    }

    /// Persistent status; missing process configuration cannot turn protection off.
    pub async fn personal_money_configuration(&self) -> Result<Option<PersonalMoneyConfiguration>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = read::<PersonalMoneyConfiguration>(
            &mut tx,
            "accounting_personal_configuration",
            "personal-money-configuration-v1",
        )
        .await?;
        if let Some(config) = &value {
            config.validate()?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// A received uncertainty receipt only records original costs. It grants no
    /// execution permission, changes no run/clock/response, and remains valid
    /// after its original lease/deadline expires. The exact committed dispatch
    /// fence must match; persisted JSON and a different attempt cannot call it.
    pub async fn note_received_model_uncertainty(
        &self,
        run_id: &str,
        attempt_id: &str,
        receipt: &InterruptedModelReceipt,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = persistence::load_locked(&mut tx)
            .await
            .map_err(accounting_error)?;
        let binding: PersonalRunBinding = read(&mut tx, "accounting_personal_runs", run_id)
            .await?
            .ok_or(Error::Forbidden)?;
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let run: StoredRun = parse(&payload.ok_or(Error::NotFound)?)?;
        let id = monetary_attempt_id(run_id, attempt_id);
        let context: AttemptBinding = read(&mut tx, "accounting_model_bindings", &id)
            .await?
            .ok_or(Error::Forbidden)?;
        let reservation = locked.ledger.reservation(&id).ok_or(Error::Forbidden)?;
        if binding.run_id() != run_id
            || binding.project_id() != run.record.spec.project_id
            || binding.run_spec_sha256() != canonical_hash(&run.record.spec)
            || context.id != id
            || context.run_id != run_id
            || context.attempt_id != attempt_id
            || canonical_hash(receipt.dispatch()) != canonical_hash(&dispatch_scope(&context)?)
            || context.admission_receipt_sha256 != canonical_hash(&reservation.receipt)
            || reservation.spec.request_sha256 != context.wire_sha256
            || !run.record.model_attempts.iter().any(|a| {
                a.id == attempt_id && a.request_sha256.as_deref() == Some(&context.logical_sha256)
            })
        {
            return Err(Error::Forbidden);
        }
        let operation_id = format!("receive-uncertainty:{id}");
        if let Some(old) = persistence::operation(&mut tx, &operation_id)
            .await
            .map_err(accounting_error)?
        {
            if old.kind != AccountingOperationKind::Uncertainty
                || old.request_sha256 != context.wire_sha256
                || canonical_hash(&old.receipt["received"]) != canonical_hash(receipt)
                || canonical_hash(&old.receipt["reservation"]) != canonical_hash(reservation)
            {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        locked
            .ledger
            .uncertain_with_usage(
                locked.ledger.revision(),
                &id,
                "provider_receive_interrupted",
                Some(receipt.usage().clone()),
            )
            .map_err(accounting_error)?;
        let reservation = locked.ledger.reservation(&id).ok_or(Error::Forbidden)?;
        let operation = AccountingOperation {
            id: operation_id,
            kind: AccountingOperationKind::Uncertainty,
            request_sha256: context.wire_sha256.clone(),
            accepted_ledger_revision: locked.ledger.revision(),
            receipt: json!({"binding":context,"complete_usage":false,"received":receipt,"charged_units":reservation.charged_units().map_err(accounting_error)?,"overrun":reservation.overrun(),"reservation":reservation}),
        };
        locked.persist(&mut tx).await.map_err(accounting_error)?;
        persistence::record_operation(&mut tx, &operation)
            .await
            .map_err(accounting_error)?;
        tx.commit().await.map_err(storage_error)
    }
    /// Exact data from the committed actual admission. This read grants no new
    /// send permit: Runtime must already own its just-admitted attempt/turn.
    pub async fn monetary_dispatch_scope(
        &self,
        run_id: &str,
        lease: &str,
        attempt_id: &str,
    ) -> Result<Option<ModelDispatchScope>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let binding: Option<PersonalRunBinding> =
            read(&mut tx, "accounting_personal_runs", run_id).await?;
        if binding.is_none() {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let run: StoredRun = parse(&payload.ok_or(Error::NotFound)?)?;
        run.assert_lease(lease, now())?;
        if run.record.id != run_id
            || run.record.state != RunState::Running
            || time_budget_exhausted(&run.record, now())
        {
            return Err(Error::BudgetExceeded);
        }
        let id = monetary_attempt_id(run_id, attempt_id);
        let context: AttemptBinding = read(&mut tx, "accounting_model_bindings", &id)
            .await?
            .ok_or(Error::Forbidden)?;
        let ledger = persistence::snapshot(&mut tx)
            .await
            .map_err(accounting_error)?;
        let reservation = ledger.reservation(&id).ok_or(Error::Forbidden)?;
        if context.id != id
            || context.run_id != run_id
            || context.attempt_id != attempt_id
            || context.lease_fence_sha256 != hash(lease.as_bytes())
            || context.admission_receipt_sha256 != canonical_hash(&reservation.receipt)
            || reservation.spec.request_sha256 != context.wire_sha256
            || !matches!(reservation.state, ReservationState::Held)
            || !run.record.model_attempts.iter().any(|a| {
                a.id == attempt_id
                    && a.state == ModelAttemptState::InFlight
                    && a.request_sha256.as_deref() == Some(&context.logical_sha256)
            })
        {
            return Err(Error::Forbidden);
        }
        let result = dispatch_scope(&context)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(result))
    }
    /// Trusted installation operator API. Do not expose as a model tool or bind
    /// its fields from ordinary client RunSpec. This first slice has immutable
    /// installation identity/policy and immutable deployment/price versions.
    pub async fn register_personal_model_deployment(
        &self,
        installation: PoolPolicy,
        deployment: OperatorModelDeployment,
        price: PriceVersion,
    ) -> Result<()> {
        if !matches!(&installation.scope, PoolScope::Installation(_))
            || deployment.id.is_empty()
            || deployment.model_sha256 != price.model_sha256
            || deployment.provider.id != price.provider_id
            || deployment.price_id != price.id
            || deployment.price_version != price.version
            || matches!(deployment.provider.kind, ProviderKind::Test)
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = persistence::load_locked(&mut tx)
            .await
            .map_err(accounting_error)?;
        immutable(
            &mut tx,
            "accounting_personal_installation",
            "personal-installation-v1",
            &installation,
        )
        .await?;
        immutable(
            &mut tx,
            "accounting_provider_deployments",
            &deployment.id,
            &deployment,
        )
        .await?;
        locked
            .ledger
            .register_price(locked.ledger.revision(), price)
            .map_err(accounting_error)?;
        locked
            .ledger
            .activate_pool(locked.ledger.revision(), installation)
            .map_err(accounting_error)?;
        locked.persist(&mut tx).await.map_err(accounting_error)?;
        tx.commit().await.map_err(storage_error)
    }

    /// Trusted operator binds actual stored personal run/project, never actor
    /// claims or arbitrary caller-required pools. Tenant/offline resolution has
    /// no supported operator API until immutable actor RunBindings exist.
    pub async fn bind_personal_run_money(
        &self,
        run_id: &str,
        deployment_id: &str,
        project_policy: PoolPolicy,
        run_policy: PoolPolicy,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = persistence::load_locked(&mut tx)
            .await
            .map_err(accounting_error)?;
        let installation: PoolPolicy = read(
            &mut tx,
            "accounting_personal_installation",
            "personal-installation-v1",
        )
        .await?
        .ok_or(Error::Forbidden)?;
        let PoolScope::Installation(installation_id) = installation.scope else {
            return Err(Error::Forbidden);
        };
        let deployment: OperatorModelDeployment =
            read(&mut tx, "accounting_provider_deployments", deployment_id)
                .await?
                .ok_or(Error::Forbidden)?;
        if deployment.id != deployment_id {
            return Err(Error::Forbidden);
        }
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let stored: StoredRun = parse(&payload.ok_or(Error::NotFound)?)?;
        let tenant: Option<String> =
            sqlx::query_scalar("SELECT id FROM tenant_projects WHERE id=$1")
                .bind(&stored.record.spec.project_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if stored.record.id != run_id
            || tenant.is_some()
            || project_policy.scope != PoolScope::Project(stored.record.spec.project_id.clone())
            || run_policy.scope != PoolScope::Run(run_id.into())
        {
            return Err(Error::Forbidden);
        }
        let binding = PersonalRunBinding::Legacy(LegacyPersonalRunBinding {
            run_id: run_id.into(),
            project_id: stored.record.spec.project_id.clone(),
            installation_id,
            deployment_id: deployment_id.into(),
            run_spec_sha256: canonical_hash(&stored.record.spec),
        });
        if let Some(old) =
            read::<PersonalRunBinding>(&mut tx, "accounting_personal_runs", run_id).await?
        {
            if canonical_hash(&old) != canonical_hash(&binding) {
                return Err(Error::Conflict);
            }
        } else if !stored.record.model_attempts.is_empty() || terminal(&stored.record.state) {
            return Err(Error::UncertainEffect);
        }
        immutable(&mut tx, "accounting_personal_runs", run_id, &binding).await?;
        for policy in [project_policy, run_policy] {
            locked
                .ledger
                .activate_pool(locked.ledger.revision(), policy)
                .map_err(accounting_error)?;
        }
        locked.persist(&mut tx).await.map_err(accounting_error)?;
        tx.commit().await.map_err(storage_error)
    }

    pub async fn monetary_ledger(&self) -> Result<MonetaryLedger> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let result = persistence::snapshot(&mut tx)
            .await
            .map_err(accounting_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }

    pub async fn admit_prepared_model_attempt<F>(
        &self,
        run: &str,
        lease: &str,
        attempt: &str,
        logical: &str,
        wire: &str,
        change: F,
    ) -> Result<StoredRun>
    where
        F: Fn(&mut StoredRun) -> Result<Change> + Send + Sync,
    {
        let intent = AccountingIntent::Admit {
            attempt_id: attempt,
            logical,
            wire,
        };
        self.mutate_with_accounting(run, Some(lease), Some(&intent), change)
            .await
    }

    pub async fn settle_received_model_attempt<F>(
        &self,
        run: &str,
        lease: &str,
        attempt: &str,
        receipt: Option<&CompletedModelReceipt>,
        change: F,
    ) -> Result<StoredRun>
    where
        F: Fn(&mut StoredRun) -> Result<Change> + Send + Sync,
    {
        let intent = receipt.map(|receipt| AccountingIntent::Settle {
            attempt_id: attempt,
            receipt,
        });
        self.mutate_with_accounting(run, Some(lease), intent.as_ref(), change)
            .await
    }

    pub(crate) async fn mutate_with_accounting<F>(
        &self,
        run: &str,
        lease: Option<&str>,
        intent: Option<&AccountingIntent<'_>>,
        change: F,
    ) -> Result<StoredRun>
    where
        F: Fn(&mut StoredRun) -> Result<Change> + Send + Sync,
    {
        Box::pin(self.mutate_with_journal(
            run,
            lease,
            workflow_clocks::WorkflowClockIntent::Execution,
            None,
            intent,
            change,
        ))
        .await
    }
}

/// Called after workflow projection, before any run/attempt/event SQL writes.
pub(crate) async fn apply(
    tx: &mut Transaction<'_, Any>,
    locked: &mut LockedLedger,
    stored: &StoredRun,
    before: &[ModelAttempt],
    change: &mut Change,
    intent: Option<&AccountingIntent<'_>>,
) -> Result<Option<AccountingOperation>> {
    let run_id = &stored.record.id;
    let Some(binding) = read::<PersonalRunBinding>(tx, "accounting_personal_runs", run_id).await?
    else {
        // Activating an installation cannot silently leave future requests in
        // free legacy mode. Historical reads remain available; new model IO
        // requires an operator-bound personal run or fails closed.
        let active: Option<PoolPolicy> = read(
            tx,
            "accounting_personal_installation",
            "personal-installation-v1",
        )
        .await?;
        if active.is_some()
            && (intent.is_some()
                || stored
                    .record
                    .model_attempts
                    .iter()
                    .any(|a| !before.iter().any(|b| b.id == a.id)))
        {
            return Err(Error::Forbidden);
        }
        return Ok(None);
    };
    if binding.run_id() != *run_id
        || binding.project_id() != stored.record.spec.project_id
        || binding.run_spec_sha256() != canonical_hash(&stored.record.spec)
    {
        return Err(Error::Forbidden);
    }
    let installation: PoolPolicy = read(
        tx,
        "accounting_personal_installation",
        "personal-installation-v1",
    )
    .await?
    .ok_or(Error::Forbidden)?;
    if installation.scope != PoolScope::Installation(binding.installation_id().into()) {
        return Err(Error::Forbidden);
    }
    let after = &stored.record.model_attempts;
    let added: Vec<_> = after
        .iter()
        .filter(|a| !before.iter().any(|b| b.id == a.id))
        .collect();
    for old in before {
        let new = after
            .iter()
            .find(|a| a.id == old.id)
            .ok_or(Error::UncertainEffect)?;
        if old.request_sha256 != new.request_sha256
            || old.reserved_tokens != new.reserved_tokens
            || old.started_at != new.started_at
        {
            return Err(Error::Forbidden);
        }
        let settling = matches!(intent, Some(AccountingIntent::Settle { attempt_id, .. }) if *attempt_id == old.id);
        if !settling
            && canonical_hash(&old.usage) != canonical_hash(&new.usage)
            && !(new.state == ModelAttemptState::Interrupted
                && old.usage.input_tokens == new.usage.input_tokens
                && old.usage.output_tokens == new.usage.output_tokens)
        {
            return Err(Error::Forbidden);
        }
        if !settling && old.state != new.state && new.state != ModelAttemptState::Interrupted {
            return Err(Error::Forbidden);
        }
    }
    let operation = match intent {
        Some(AccountingIntent::Admit {
            attempt_id,
            logical,
            wire,
        }) => {
            if added.len() != 1
                || added[0].id != *attempt_id
                || added[0].state != ModelAttemptState::InFlight
                || added[0].request_sha256.as_deref() != Some(*logical)
            {
                return Err(Error::UncertainEffect);
            }
            let model_spec = if let Some(write) = &change.workflow_write {
                &crate::workflows::sampling_model_context(&write.next, attempt_id)?.spec
            } else {
                &stored.record.spec
            };
            let profile = model_spec.provider.as_ref().ok_or(Error::Forbidden)?;
            let deployment = selected_deployment(tx, &binding, profile).await?;
            if canonical_hash(profile) != canonical_hash(&deployment.provider)
                || model_spec.project_id != binding.project_id()
                || model_spec.order_id != stored.record.spec.order_id
            {
                return Err(Error::Forbidden);
            }
            let exact_count = if deployment.token_mode == MonetaryTokenMode::ExactArtifactRenderer {
                let model = crate::model_registry::check_binding(tx, model_spec)
                    .await?
                    .ok_or(Error::Forbidden)?;
                if model.identity().model_sha256 != deployment.model_sha256 {
                    return Err(Error::Forbidden);
                }
                let messages = if let Some(write) = &change.workflow_write {
                    &crate::workflows::sampling_model_context(&write.next, attempt_id)?.messages
                } else {
                    &stored.checkpoint.messages
                };
                let tools = model_tool_catalog(model_spec);
                let prompt = model
                    .prompt(
                        messages,
                        &tools,
                        &serde_json::to_value(&model_spec.output).map_err(storage_error)?,
                        logical,
                    )
                    .map_err(accounting_error)?;
                Some(
                    model
                        .tokenizer
                        .count(&prompt, u64::from(model_spec.budgets.max_output_tokens))
                        .map_err(accounting_error)?,
                )
            } else {
                None
            };
            let scopes = vec![
                PoolScope::Run(run_id.clone()),
                PoolScope::Project(binding.project_id().into()),
                PoolScope::Installation(binding.installation_id().into()),
            ];
            let mut pools: Vec<_> = scopes
                .iter()
                .map(|scope| locked.ledger.active_binding(scope))
                .collect::<std::result::Result<_, _>>()
                .map_err(accounting_error)?;
            pools.sort();
            let id = monetary_attempt_id(run_id, attempt_id);
            let spec = ReservationSpec {
                attempt_id: id.clone(),
                request_sha256: (*wire).into(),
                provider_id: profile.id.clone(),
                model_sha256: deployment.model_sha256.clone(),
                price_id: deployment.price_id.clone(),
                price_version: deployment.price_version,
                input_upper_bound: exact_count
                    .as_ref()
                    .map(|c| c.input_tokens)
                    .unwrap_or(u64::from(profile.context_tokens)),
                output_upper_bound: u64::from(model_spec.budgets.max_output_tokens),
                pools,
            };
            let admitted = locked
                .ledger
                .admit(locked.ledger.revision(), spec, &scopes)
                .map_err(accounting_error)?;
            if !admitted.newly_reserved {
                return Err(Error::UncertainEffect);
            }
            let context = AttemptBinding {
                id: id.clone(),
                run_id: run_id.clone(),
                attempt_id: (*attempt_id).into(),
                logical_sha256: (*logical).into(),
                wire_sha256: (*wire).into(),
                provider_profile_sha256: canonical_hash(profile),
                model_spec_sha256: canonical_hash(model_spec),
                deployment_sha256: canonical_hash(&deployment),
                admission_receipt_sha256: canonical_hash(&admitted.receipt),
                lease_fence_sha256: hash(
                    stored
                        .lease
                        .as_ref()
                        .ok_or(Error::StaleLease)?
                        .token
                        .as_bytes(),
                ),
            };
            immutable(tx, "accounting_model_bindings", &id, &context).await?;
            if let Some(write) = &change.workflow_write {
                let deadline =
                    crate::workflows::model_admission_deadline(stored, write, attempt_id)?;
                change.workflow_tool_deadline = Some(
                    change
                        .workflow_tool_deadline
                        .map_or(deadline, |old| old.min(deadline)),
                );
            }
            change.events.push(("monetary_model_reserved".into(), json!({"format":1,"attempt_id":attempt_id,"accounting_id":id,"logical_sha256":logical,"wire_sha256":wire,"token_mode":deployment.token_mode,"exact_tokenizer_qualified":exact_count.is_some(),"token_count":exact_count,"receipt":admitted.receipt})));
            Some(AccountingOperation {
                id: format!("admission:{id}"),
                kind: AccountingOperationKind::Admission,
                request_sha256: (*wire).into(),
                accepted_ledger_revision: locked.ledger.revision(),
                receipt: json!({"binding":context,"admission":admitted.receipt,"token_mode":deployment.token_mode}),
            })
        }
        Some(AccountingIntent::Settle {
            attempt_id,
            receipt,
        }) => {
            if !added.is_empty() {
                return Err(Error::Forbidden);
            }
            let id = monetary_attempt_id(run_id, attempt_id);
            let context: AttemptBinding = read(tx, "accounting_model_bindings", &id)
                .await?
                .ok_or(Error::Forbidden)?;
            let model_spec = if let Some(write) = &change.workflow_write {
                &crate::workflows::sampling_model_context(&write.next, attempt_id)?.spec
            } else {
                &stored.record.spec
            };
            let deployment = selected_deployment(
                tx,
                &binding,
                model_spec.provider.as_ref().ok_or(Error::Forbidden)?,
            )
            .await?;
            if context.id != id
                || context.run_id != *run_id
                || context.attempt_id != *attempt_id
                || context.logical_sha256 != receipt.logical_sha256()
                || context.wire_sha256 != receipt.wire_sha256()
                || context.provider_profile_sha256 != receipt.provider_profile_sha256()
                || context.deployment_sha256 != canonical_hash(&deployment)
                || canonical_hash(receipt.dispatch()) != canonical_hash(&dispatch_scope(&context)?)
            {
                return Err(Error::Forbidden);
            }
            let old = before
                .iter()
                .find(|a| a.id == *attempt_id)
                .ok_or(Error::Forbidden)?;
            let new = after
                .iter()
                .find(|a| a.id == *attempt_id)
                .ok_or(Error::Forbidden)?;
            if old.state != ModelAttemptState::InFlight
                || new.state == ModelAttemptState::InFlight
                || new.usage.input_tokens != receipt.usage().input_tokens
                || new.usage.output_tokens != receipt.usage().output_tokens
            {
                return Err(Error::Forbidden);
            }
            let reservation = locked.ledger.reservation(&id).ok_or(Error::Forbidden)?;
            if context.admission_receipt_sha256 != canonical_hash(&reservation.receipt) {
                return Err(Error::Forbidden);
            }
            let usage = receipt.usage();
            let complete = usage.input_tokens.is_some()
                && usage.output_tokens.is_some()
                && usage.cached_input_tokens.is_some()
                && !(usage.cached_input_tokens.unwrap_or(0) > 0
                    && reservation.price.cached_input_units_per_million.is_none());
            if complete {
                locked
                    .ledger
                    .settle(locked.ledger.revision(), &id, usage.clone())
                    .map_err(accounting_error)?;
            } else {
                locked
                    .ledger
                    .uncertain_with_usage(
                        locked.ledger.revision(),
                        &id,
                        "provider_usage_incomplete",
                        Some(usage.clone()),
                    )
                    .map_err(accounting_error)?;
            }
            let reservation = locked.ledger.reservation(&id).ok_or(Error::Forbidden)?;
            let charged_units = reservation.charged_units().map_err(accounting_error)?;
            change.events.push(("monetary_model_response".into(), json!({"format":1,"attempt_id":attempt_id,"accounting_id":id,"complete_usage":complete,"charged_units":charged_units,"overrun":reservation.overrun(),"received":receipt,"reservation":reservation})));
            Some(AccountingOperation {
                id: format!("response:{id}"),
                kind: if complete {
                    AccountingOperationKind::Settlement
                } else {
                    AccountingOperationKind::Uncertainty
                },
                request_sha256: context.wire_sha256.clone(),
                accepted_ledger_revision: locked.ledger.revision(),
                receipt: json!({"binding":context,"complete_usage":complete,"charged_units":charged_units,"overrun":reservation.overrun(),"received":receipt,"reservation":reservation}),
            })
        }
        None if !added.is_empty() => return Err(Error::Forbidden),
        None => None,
    };
    Ok(operation)
}
