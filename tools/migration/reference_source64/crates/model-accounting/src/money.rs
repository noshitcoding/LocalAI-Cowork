//! Immutable prices and checked integer reservation transitions.
use crate::{Error, Result, digest, nonempty, valid_hash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Currency {
    pub code: String,
    /// Units are 10^-scale of this currency. No binary floating point is used.
    pub scale: u8,
}

impl Currency {
    pub fn validate(&self) -> Result<()> {
        if self.code.len() != 3
            || !self.code.bytes().all(|b| b.is_ascii_uppercase())
            || self.scale > 9
        {
            return Err(Error::Invalid("invalid currency/scale".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceVersion {
    pub id: String,
    pub version: u64,
    pub provider_id: String,
    pub model_sha256: String,
    pub currency: Currency,
    pub input_units_per_million: u64,
    pub output_units_per_million: u64,
    pub cached_input_units_per_million: Option<u64>,
    pub source_sha256: String,
}

impl PriceVersion {
    pub fn validate(&self) -> Result<()> {
        self.currency.validate()?;
        if !nonempty(&self.id)
            || self.version == 0
            || !nonempty(&self.provider_id)
            || !valid_hash(&self.model_sha256)
            || !valid_hash(&self.source_sha256)
        {
            return Err(Error::Invalid("invalid immutable price identity".into()));
        }
        Ok(())
    }

    /// Ceiling only once after summing exact per-million rational components.
    /// Cached input is a subset of input, never additional input tokens.
    pub fn cost(&self, input: u64, output: u64, cached_input: u64) -> Result<u64> {
        self.validate()?;
        if cached_input > input {
            return Err(Error::Invalid("cached input exceeds total input".into()));
        }
        let cached_rate = match (cached_input, self.cached_input_units_per_million) {
            (0, None) => 0,
            (_, Some(rate)) => rate,
            _ => return Err(Error::Invalid("cached billing is not priced".into())),
        };
        let uncached = u128::from(input - cached_input) * u128::from(self.input_units_per_million);
        let cached = u128::from(cached_input) * u128::from(cached_rate);
        let output = u128::from(output) * u128::from(self.output_units_per_million);
        let numerator = uncached
            .checked_add(cached)
            .and_then(|sum| sum.checked_add(output))
            .ok_or(Error::Overflow)?;
        let units = numerator / 1_000_000 + u128::from(numerator % 1_000_000 != 0);
        u64::try_from(units).map_err(|_| Error::Overflow)
    }

    pub fn reserve(&self, input_bound: u64, output_bound: u64) -> Result<u64> {
        // Reservation never assumes a discount or known cache hit.
        let mut worst = self.clone();
        worst.input_units_per_million = self
            .input_units_per_million
            .max(self.cached_input_units_per_million.unwrap_or(0));
        worst.cost(input_bound, output_bound, 0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PoolScope {
    Run(String),
    Project(String),
    Organization(String),
    Instance(String),
    OfflineDevice(String),
    /// One operator-owned installation; distinct from a tenant instance.
    Installation(String),
}

impl PoolScope {
    fn validate(&self) -> Result<()> {
        let id = match self {
            Self::Run(id)
            | Self::Project(id)
            | Self::Organization(id)
            | Self::Instance(id)
            | Self::OfflineDevice(id)
            | Self::Installation(id) => id,
        };
        if !nonempty(id) {
            return Err(Error::Invalid("invalid budget scope".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolPolicy {
    pub scope: PoolScope,
    pub version: u64,
    pub currency: Currency,
    pub limit_units: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolBinding {
    pub scope: PoolScope,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationSpec {
    pub attempt_id: String,
    pub request_sha256: String,
    pub provider_id: String,
    pub model_sha256: String,
    pub price_id: String,
    pub price_version: u64,
    pub input_upper_bound: u64,
    pub output_upper_bound: u64,
    /// Derived by trusted scope resolution, never accepted as a model grant.
    pub pools: Vec<PoolBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub provider_receipt_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReservationState {
    Held,
    Uncertain {
        reason: String,
        /// Received partial usage never grants a release. Omission preserves
        /// the serialization of historical uncertainty without a receipt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<ProviderUsage>,
    },
    Settled {
        usage: ProviderUsage,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationReceipt {
    pub specification_sha256: String,
    pub price_sha256: String,
    pub reserved_units: u64,
    pub accepted_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub spec: ReservationSpec,
    pub price: PriceVersion,
    pub receipt: ReservationReceipt,
    pub state: ReservationState,
}

impl Reservation {
    pub fn complete_usage(&self) -> bool {
        let ReservationState::Settled { usage } = &self.state else {
            return false;
        };
        usage.input_tokens.is_some()
            && usage.output_tokens.is_some()
            && usage
                .provider_receipt_sha256
                .as_deref()
                .is_some_and(valid_hash)
            && usage.cached_input_tokens.is_some()
            && !(usage.cached_input_tokens.unwrap_or(0) > 0
                && self.price.cached_input_units_per_million.is_none())
    }

    pub fn charged_units(&self) -> Result<u64> {
        let Some(usage) = self.received_usage() else {
            return Ok(self.receipt.reserved_units);
        };
        if usage
            .provider_receipt_sha256
            .as_ref()
            .is_some_and(|hash| !valid_hash(hash))
        {
            return Err(Error::Invalid("invalid provider usage receipt".into()));
        }
        if usage
            .cached_input_tokens
            .zip(usage.input_tokens)
            .is_some_and(|(cached, input)| cached > input)
        {
            return Err(Error::Invalid("cached usage exceeds measured input".into()));
        }
        // A known cache count establishes at least that much total input.
        // Partial evidence never assumes a cache discount or releases a hold.
        let input = usage
            .input_tokens
            .unwrap_or(0)
            .max(usage.cached_input_tokens.unwrap_or(0));
        let output = usage.output_tokens.unwrap_or(0);
        if self.complete_usage() {
            self.price
                .cost(input, output, usage.cached_input_tokens.unwrap_or(0))
        } else {
            Ok(self
                .price
                .reserve(input, output)?
                .max(self.receipt.reserved_units))
        }
    }

    fn received_usage(&self) -> Option<&ProviderUsage> {
        match &self.state {
            ReservationState::Settled { usage } => Some(usage),
            ReservationState::Uncertain { evidence, .. } => evidence.as_ref(),
            ReservationState::Held => None,
        }
    }

    pub fn overrun(&self) -> bool {
        let Some(usage) = self.received_usage() else {
            return false;
        };
        usage
            .input_tokens
            .is_some_and(|n| n > self.spec.input_upper_bound)
            || usage
                .output_tokens
                .is_some_and(|n| n > self.spec.output_upper_bound)
            || usage
                .cached_input_tokens
                .is_some_and(|n| n > self.spec.input_upper_bound)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
    pub receipt: ReservationReceipt,
    /// Only true for the new atomic reservation; replays never grant dispatch.
    pub newly_reserved: bool,
}

/// Serializable transaction state. Storage must lock it, verify its revision,
/// apply one transition and persist it with run/attempt/lease events atomically.
/// All balances are derived; there is no mutable aggregate that can lose holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonetaryLedger {
    revision: u64,
    prices: Vec<PriceVersion>,
    policies: Vec<PoolPolicy>,
    active_policies: Vec<PoolBinding>,
    reservations: BTreeMap<String, Reservation>,
}

impl MonetaryLedger {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn reservation(&self, id: &str) -> Option<&Reservation> {
        self.reservations.get(id)
    }
    pub fn reservations(&self) -> &BTreeMap<String, Reservation> {
        &self.reservations
    }
    pub fn active_binding(&self, scope: &PoolScope) -> Result<PoolBinding> {
        self.active_policies
            .iter()
            .find(|binding| &binding.scope == scope)
            .cloned()
            .ok_or(Error::Conflict)
    }
    /// Typed policy reads preserve the distinction between global scopes and
    /// the integrating application's own immutable installation identity.
    pub fn active_policy(&self, scope: &PoolScope) -> Result<&PoolPolicy> {
        let binding = self.active_binding(scope)?;
        self.policies
            .iter()
            .find(|policy| policy.scope == binding.scope && policy.version == binding.version)
            .ok_or(Error::Conflict)
    }
    pub fn has_active_installation(&self) -> bool {
        self.active_policies
            .iter()
            .any(|binding| matches!(binding.scope, PoolScope::Installation(_)))
    }
    fn advance(&mut self, expected: u64) -> Result<()> {
        if self.revision != expected {
            return Err(Error::StaleRevision);
        }
        self.revision = self.revision.checked_add(1).ok_or(Error::Overflow)?;
        Ok(())
    }

    pub fn register_price(&mut self, expected: u64, price: PriceVersion) -> Result<()> {
        price.validate()?;
        if let Some(old) = self
            .prices
            .iter()
            .find(|old| old.id == price.id && old.version == price.version)
        {
            return if *old == price {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        self.advance(expected)?;
        self.prices.push(price);
        Ok(())
    }

    pub fn activate_pool(&mut self, expected: u64, policy: PoolPolicy) -> Result<()> {
        policy.scope.validate()?;
        policy.currency.validate()?;
        if policy.version == 0 {
            return Err(Error::Invalid("zero policy version".into()));
        }
        if self
            .policies
            .iter()
            .any(|old| old.scope == policy.scope && old.currency != policy.currency)
        {
            return Err(Error::Conflict); // No implicit exchange-rate conversion.
        }
        if let Some(old) = self
            .policies
            .iter()
            .find(|old| old.scope == policy.scope && old.version == policy.version)
            && *old != policy
        {
            return Err(Error::Conflict);
        }
        let binding = PoolBinding {
            scope: policy.scope.clone(),
            version: policy.version,
        };
        if self.active_policies.contains(&binding) {
            return Ok(());
        }
        if self
            .active_policies
            .iter()
            .any(|old| old.scope == binding.scope && old.version >= binding.version)
        {
            return Err(Error::Conflict);
        }
        self.advance(expected)?;
        if !self
            .policies
            .iter()
            .any(|old| old.scope == policy.scope && old.version == policy.version)
        {
            self.policies.push(policy);
        }
        self.active_policies
            .retain(|old| old.scope != binding.scope);
        self.active_policies.push(binding);
        self.active_policies.sort();
        Ok(())
    }

    pub fn charged_to(&self, scope: &PoolScope) -> Result<u64> {
        self.reservations
            .values()
            .filter(|r| r.spec.pools.iter().any(|p| &p.scope == scope))
            .try_fold(0u64, |total, reservation| {
                total
                    .checked_add(reservation.charged_units()?)
                    .ok_or(Error::Overflow)
            })
    }

    pub fn admit(
        &mut self,
        expected: u64,
        spec: ReservationSpec,
        required_scopes: &[PoolScope],
    ) -> Result<Admission> {
        let specification_sha256 = digest(&spec)?;
        if let Some(old) = self.reservations.get(&spec.attempt_id) {
            if old.receipt.specification_sha256 != specification_sha256 {
                return Err(Error::Conflict);
            }
            return Ok(Admission {
                receipt: old.receipt.clone(),
                newly_reserved: false,
            });
        }
        if !nonempty(&spec.attempt_id)
            || !valid_hash(&spec.request_sha256)
            || !valid_hash(&spec.model_sha256)
            || !nonempty(&spec.provider_id)
            || spec.pools.is_empty()
        {
            return Err(Error::Invalid("invalid reservation specification".into()));
        }
        let requested: BTreeSet<_> = spec
            .pools
            .iter()
            .map(|binding| binding.scope.clone())
            .collect();
        let required: BTreeSet<_> = required_scopes.iter().cloned().collect();
        if requested != required
            || requested.len() != spec.pools.len()
            || required.len() != required_scopes.len()
            || !required
                .iter()
                .any(|scope| matches!(scope, PoolScope::Run(_)))
            || spec.pools.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(Error::Conflict);
        }
        let price = self
            .prices
            .iter()
            .find(|price| price.id == spec.price_id && price.version == spec.price_version)
            .cloned()
            .ok_or(Error::PriceUnknown)?;
        if price.model_sha256 != spec.model_sha256 || price.provider_id != spec.provider_id {
            return Err(Error::Conflict);
        }
        let reserved_units = price.reserve(spec.input_upper_bound, spec.output_upper_bound)?;
        for binding in &spec.pools {
            binding.scope.validate()?;
            if !self.active_policies.contains(binding) {
                return Err(Error::Conflict);
            }
            let policy = self
                .policies
                .iter()
                .find(|policy| policy.scope == binding.scope && policy.version == binding.version)
                .ok_or(Error::Conflict)?;
            if policy.currency != price.currency {
                return Err(Error::Conflict);
            }
            if self
                .reservations
                .values()
                .any(|r| r.spec.pools.iter().any(|p| p.scope == binding.scope) && r.overrun())
                || self
                    .charged_to(&binding.scope)?
                    .checked_add(reserved_units)
                    .ok_or(Error::Overflow)?
                    > policy.limit_units
            {
                return Err(Error::BudgetExceeded);
            }
        }
        self.advance(expected)?;
        let receipt = ReservationReceipt {
            specification_sha256,
            price_sha256: digest(&price)?,
            reserved_units,
            accepted_revision: self.revision,
        };
        self.reservations.insert(
            spec.attempt_id.clone(),
            Reservation {
                spec,
                price,
                receipt: receipt.clone(),
                state: ReservationState::Held,
            },
        );
        Ok(Admission {
            receipt,
            newly_reserved: true,
        })
    }

    pub fn uncertain(&mut self, expected: u64, id: &str, reason: &str) -> Result<()> {
        self.uncertain_with_usage(expected, id, reason, None)
    }

    pub fn uncertain_with_usage(
        &mut self,
        expected: u64,
        id: &str,
        reason: &str,
        evidence: Option<ProviderUsage>,
    ) -> Result<()> {
        if !nonempty(reason) {
            return Err(Error::Invalid("invalid uncertainty reason".into()));
        }
        let old = self.reservations.get(id).ok_or(Error::Conflict)?;
        let mut next = old.clone();
        next.state = ReservationState::Uncertain {
            reason: reason.into(),
            evidence,
        };
        if old.state == next.state {
            return Ok(());
        }
        if old.state != ReservationState::Held {
            return Err(Error::Conflict);
        }
        next.charged_units()?;
        self.advance(expected)?;
        self.reservations.insert(id.into(), next);
        Ok(())
    }

    pub fn settle(&mut self, expected: u64, id: &str, usage: ProviderUsage) -> Result<()> {
        let old = self.reservations.get(id).ok_or(Error::Conflict)?;
        if let ReservationState::Settled { usage: old_usage } = &old.state {
            return if *old_usage == usage {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        let mut next = old.clone();
        next.state = ReservationState::Settled { usage };
        next.charged_units()?;
        self.advance(expected)?;
        self.reservations.insert(id.into(), next);
        Ok(())
    }

    /// Required after deserialization before applying any new IO admission.
    pub fn validate(&self) -> Result<()> {
        let mut price_ids = BTreeSet::new();
        for price in &self.prices {
            price.validate()?;
            if !price_ids.insert((&price.id, price.version)) {
                return Err(Error::Conflict);
            }
        }
        let mut policy_ids = BTreeSet::new();
        let mut currencies = BTreeMap::new();
        for policy in &self.policies {
            policy.scope.validate()?;
            policy.currency.validate()?;
            if policy.version == 0 || !policy_ids.insert((&policy.scope, policy.version)) {
                return Err(Error::Conflict);
            }
            if currencies
                .insert(&policy.scope, &policy.currency)
                .is_some_and(|old| old != &policy.currency)
            {
                return Err(Error::Conflict);
            }
        }
        let mut active = BTreeSet::new();
        for binding in &self.active_policies {
            if !active.insert(&binding.scope)
                || !policy_ids.contains(&(&binding.scope, binding.version))
            {
                return Err(Error::Conflict);
            }
        }
        for (id, r) in &self.reservations {
            if !nonempty(id)
                || !valid_hash(&r.spec.request_sha256)
                || r.spec.pools.windows(2).any(|pair| pair[0] >= pair[1])
                || matches!(&r.state, ReservationState::Uncertain {reason, ..} if !nonempty(reason))
                || id != &r.spec.attempt_id
                || r.receipt.specification_sha256 != digest(&r.spec)?
                || r.receipt.price_sha256 != digest(&r.price)?
                || r.receipt.reserved_units
                    != r.price
                        .reserve(r.spec.input_upper_bound, r.spec.output_upper_bound)?
                || r.receipt.accepted_revision > self.revision
                || r.receipt.accepted_revision == 0
                || !self.prices.contains(&r.price)
                || r.price.model_sha256 != r.spec.model_sha256
                || r.price.provider_id != r.spec.provider_id
                || r.spec.price_id != r.price.id
                || r.spec.price_version != r.price.version
            {
                return Err(Error::Conflict);
            }
            let mut scopes = BTreeSet::new();
            for binding in &r.spec.pools {
                if !scopes.insert(&binding.scope)
                    || !self.policies.iter().any(|policy| {
                        policy.scope == binding.scope
                            && policy.version == binding.version
                            && policy.currency == r.price.currency
                    })
                {
                    return Err(Error::Conflict);
                }
            }
            if !scopes
                .iter()
                .any(|scope| matches!(scope, PoolScope::Run(_)))
            {
                return Err(Error::Conflict);
            }
            r.charged_units()?;
        }
        Ok(())
    }
}
