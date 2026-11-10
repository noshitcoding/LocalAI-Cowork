//! Read-only personal installation money status. No operator input or IO permit.
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;

/// Exact unsigned units and versions in JSON; JavaScript must never round them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecimalU64(u64);

impl From<u64> for DecimalU64 {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
impl DecimalU64 {
    pub fn value(self) -> u64 {
        self.0
    }
}
impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom("expected canonical decimal u64"));
        }
        Ok(Self(value))
    }
}
impl JsonSchema for DecimalU64 {
    fn schema_name() -> Cow<'static, str> {
        "DecimalU64".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        // A lexicographic bound also validates MAX_u64 in the generated browser
        // validator. The final lookahead excludes a trailing line terminator.
        let maximum = u64::MAX.to_string();
        let mut terms = vec!["0".to_string(), "[1-9][0-9]{0,18}".to_string()];
        for (index, digit) in maximum.bytes().enumerate() {
            let low = u8::from(index == 0);
            let Some(high) = (digit - b'0').checked_sub(1) else {
                continue;
            };
            if low > high {
                continue;
            }
            let choice = if low == high {
                low.to_string()
            } else {
                format!("[{low}-{high}]")
            };
            let remaining = maximum.len() - index - 1;
            let suffix = if remaining == 0 {
                String::new()
            } else {
                format!("[0-9]{{{remaining}}}")
            };
            terms.push(format!("{}{choice}{suffix}", &maximum[..index]));
        }
        terms.push(maximum);
        json_schema!({"type": "string", "pattern": format!("^({})$(?![\\s\\S])", terms.join("|"))})
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalMoneyCurrency {
    #[schemars(length(min = 3, max = 3), regex(pattern = "^[A-Z]{3}$"))]
    pub code: String,
    #[schemars(range(min = 0, max = 9))]
    pub scale: u8,
}
impl<'de> Deserialize<'de> for PersonalMoneyCurrency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            code: String,
            scale: u8,
        }
        let value = Wire::deserialize(deserializer)?;
        if value.code.len() != 3
            || !value.code.bytes().all(|b| b.is_ascii_uppercase())
            || value.scale > 9
        {
            return Err(serde::de::Error::custom("invalid money currency"));
        }
        Ok(Self {
            code: value.code,
            scale: value.scale,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalMoneyPoolStatus {
    pub currency: PersonalMoneyCurrency,
    pub policy_version: DecimalU64,
    pub limit_units: DecimalU64,
    /// Confirmed measurement plus pending and conservative unknown exposure.
    pub charged_units: DecimalU64,
    pub confirmed_units: DecimalU64,
    pub pending_units: DecimalU64,
    pub unknown_units: DecimalU64,
    pub remaining_units: DecimalU64,
    /// Counts keep zero-priced incomplete attempts visible.
    pub confirmed_attempts: DecimalU64,
    pub pending_attempts: DecimalU64,
    pub unknown_attempts: DecimalU64,
    /// Received tokens exceeded an original reservation bound.
    pub overrun: bool,
    pub limit_exceeded: bool,
    pub no_headroom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalMoneyDefaults {
    pub project_limit_units: DecimalU64,
    pub run_limit_units: DecimalU64,
    pub pool_policy_version: DecimalU64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersonalMoneyStatus {
    NotConfigured {},
    /// Historical explicitly bound runs remain protected without shared config.
    LegacyManual {
        installation: PersonalMoneyPoolStatus,
    },
    OperatorConfiguration {
        installation: PersonalMoneyPoolStatus,
        defaults: PersonalMoneyDefaults,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_units_preserve_full_u64_and_reject_noncanonical_json() {
        for value in [0, 9_007_199_254_740_993, u64::MAX] {
            let encoded = serde_json::to_string(&DecimalU64::from(value)).unwrap();
            assert_eq!(encoded, format!("\"{value}\""));
            assert_eq!(
                serde_json::from_str::<DecimalU64>(&encoded)
                    .unwrap()
                    .value(),
                value
            );
        }
        for input in [
            "0",
            "1.0",
            "null",
            "\"18446744073709551616\"",
            "\"01\"",
            "\"+1\"",
            "\"-1\"",
            "\"1e3\"",
            "\"1\\n\"",
        ] {
            assert!(
                serde_json::from_str::<DecimalU64>(input).is_err(),
                "{input}"
            );
        }
    }
    #[test]
    fn absent_configuration_is_closed_and_currency_is_strict() {
        assert_eq!(
            serde_json::to_value(PersonalMoneyStatus::NotConfigured {}).unwrap(),
            serde_json::json!({"origin":"not_configured"})
        );
        assert!(
            serde_json::from_value::<PersonalMoneyStatus>(
                serde_json::json!({"origin":"not_configured","ledger_revision":"1"})
            )
            .is_err()
        );
        for value in [
            serde_json::json!({"code":"usd","scale":6}),
            serde_json::json!({"code":"USD","scale":10}),
            serde_json::json!({"code":"USD","scale":6,"endpoint":"private"}),
        ] {
            assert!(serde_json::from_value::<PersonalMoneyCurrency>(value).is_err());
        }
    }
}
