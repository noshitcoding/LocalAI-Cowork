//! Persistent tenant identity and authorization services.
//!
//! The single-owner daemon token is not a tenant identity or content grant.
pub mod audit;
pub mod audit_checkpoints;
pub mod audit_insights;
pub mod audit_origin;
pub mod audit_recovery;
pub mod audit_trust;
pub mod audit_v1;
pub mod http;
pub mod migrations;
mod model;
mod store;
pub use model::*;
pub use store::{
    IdentityStore, LockedIdentityQuota, MigrationIdentityBinding, MigrationIdentityMapping,
    MigrationIdentityTarget, MigrationProjectGrant, OidcSessionReceipt, StorageImportCharge,
    VerifiedPassword,
};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("Invalid identity request")]
    Validation,
    #[error("Authentication required")]
    Unauthenticated,
    #[error("Access denied")]
    Forbidden,
    #[error("Scoped object not found")]
    NotFound,
    #[error("Revision or immutable request conflict")]
    Conflict,
    #[error("Quota exhausted")]
    QuotaExceeded,
    #[error("Identity authorization clock moved backwards")]
    ClockRollback,
    #[error("Caller identity transaction must be rolled back")]
    TransactionPoisoned,
    #[error("Identity storage failed")]
    Storage(#[source] sqlx::Error),
    #[error("Stored identity data is invalid")]
    Corrupt,
    #[error("Original native audit key is unavailable")]
    NativeAuditKeyUnavailable,
}
impl From<sqlx::Error> for IdentityError {
    fn from(error: sqlx::Error) -> Self {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Self::Conflict
        } else {
            Self::Storage(error)
        }
    }
}
pub type Result<T> = std::result::Result<T, IdentityError>;
