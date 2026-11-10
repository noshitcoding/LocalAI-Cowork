//! Exact artifact-bound tokenization, explicit context compaction and integer accounting.
pub mod context;
pub mod money;
pub mod persistence;
pub mod received;
pub mod tokenizer;
pub mod registry;

use serde::Serialize;
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid accounting evidence: {0}")]
    Invalid(String),
    #[error("tokenizer/model qualification does not match")]
    UnqualifiedTokenizer,
    #[error("context exceeds the frozen model limit")]
    ContextLimit,
    #[error("immutable accounting identity conflicts")]
    Conflict,
    #[error("accounting revision is stale")]
    StaleRevision,
    #[error("checked accounting arithmetic overflow")]
    Overflow,
    #[error("immutable price version is unavailable")]
    PriceUnknown,
    #[error("insufficient monetary capacity")]
    BudgetExceeded,
    #[error("accounting storage failed: {0}")]
    Storage(String),
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn digest<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256(&bytes))
        .map_err(|error| Error::Invalid(error.to_string()))
}

pub(crate) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn nonempty(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
