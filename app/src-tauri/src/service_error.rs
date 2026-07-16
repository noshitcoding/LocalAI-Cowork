#![allow(dead_code)]

use crate::context::{RequestId, TenantId, WorkspaceId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceErrorCode {
    Unauthenticated,
    PermissionDenied,
    TenantRequired,
    WorkspaceRequired,
    TenantNotFound,
    WorkspaceNotFound,
    NotFound,
    ValidationFailed,
    Conflict,
    IdempotencyConflict,
    CapabilityNotAvailable,
    DesktopAgentOffline,
    DesktopAgentNotPaired,
    RateLimited,
    ServerBusy,
    InternalError,
}

impl ServiceErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::PermissionDenied => "permission_denied",
            Self::TenantRequired => "tenant_required",
            Self::WorkspaceRequired => "workspace_required",
            Self::TenantNotFound => "tenant_not_found",
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NotFound => "not_found",
            Self::ValidationFailed => "validation_failed",
            Self::Conflict => "conflict",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::CapabilityNotAvailable => "capability_not_available",
            Self::DesktopAgentOffline => "desktop_agent_offline",
            Self::DesktopAgentNotPaired => "desktop_agent_not_paired",
            Self::RateLimited => "rate_limited",
            Self::ServerBusy => "server_busy",
            Self::InternalError => "internal_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    Unauthenticated,
    Forbidden {
        reason: Option<String>,
    },
    TenantRequired,
    WorkspaceRequired,
    TenantNotFound {
        tenant_id: Option<TenantId>,
    },
    WorkspaceNotFound {
        workspace_id: Option<WorkspaceId>,
    },
    NotFound {
        resource: String,
    },
    Validation {
        message: String,
        field: Option<String>,
    },
    Conflict {
        message: String,
    },
    IdempotencyConflict,
    CapabilityNotAvailable {
        capability: String,
    },
    DesktopAgentOffline,
    DesktopAgentNotPaired,
    RateLimited,
    ServerBusy,
    Internal {
        source: String,
    },
}

impl ServiceError {
    pub fn unauthenticated() -> Self {
        Self::Unauthenticated
    }

    pub fn forbidden() -> Self {
        Self::Forbidden { reason: None }
    }

    pub fn forbidden_with_message(message: impl Into<String>) -> Self {
        Self::Forbidden {
            reason: Some(message.into()),
        }
    }

    pub fn tenant_required() -> Self {
        Self::TenantRequired
    }

    pub fn workspace_required() -> Self {
        Self::WorkspaceRequired
    }

    pub fn tenant_not_found(tenant_id: Option<TenantId>) -> Self {
        Self::TenantNotFound { tenant_id }
    }

    pub fn workspace_not_found(workspace_id: Option<WorkspaceId>) -> Self {
        Self::WorkspaceNotFound { workspace_id }
    }

    pub fn not_found(resource: impl Into<String>) -> Self {
        Self::NotFound {
            resource: resource.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
            field: None,
        }
    }

    pub fn field_validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
            field: Some(field.into()),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    pub fn idempotency_conflict() -> Self {
        Self::IdempotencyConflict
    }

    pub fn capability_not_available(capability: impl Into<String>) -> Self {
        Self::CapabilityNotAvailable {
            capability: capability.into(),
        }
    }

    pub fn desktop_agent_offline() -> Self {
        Self::DesktopAgentOffline
    }

    pub fn desktop_agent_not_paired() -> Self {
        Self::DesktopAgentNotPaired
    }

    pub fn rate_limited() -> Self {
        Self::RateLimited
    }

    pub fn server_busy() -> Self {
        Self::ServerBusy
    }

    pub fn internal(source: impl fmt::Display) -> Self {
        Self::Internal {
            source: source.to_string(),
        }
    }

    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::Unauthenticated => ServiceErrorCode::Unauthenticated,
            Self::Forbidden { .. } => ServiceErrorCode::PermissionDenied,
            Self::TenantRequired => ServiceErrorCode::TenantRequired,
            Self::WorkspaceRequired => ServiceErrorCode::WorkspaceRequired,
            Self::TenantNotFound { .. } => ServiceErrorCode::TenantNotFound,
            Self::WorkspaceNotFound { .. } => ServiceErrorCode::WorkspaceNotFound,
            Self::NotFound { .. } => ServiceErrorCode::NotFound,
            Self::Validation { .. } => ServiceErrorCode::ValidationFailed,
            Self::Conflict { .. } => ServiceErrorCode::Conflict,
            Self::IdempotencyConflict => ServiceErrorCode::IdempotencyConflict,
            Self::CapabilityNotAvailable { .. } => ServiceErrorCode::CapabilityNotAvailable,
            Self::DesktopAgentOffline => ServiceErrorCode::DesktopAgentOffline,
            Self::DesktopAgentNotPaired => ServiceErrorCode::DesktopAgentNotPaired,
            Self::RateLimited => ServiceErrorCode::RateLimited,
            Self::ServerBusy => ServiceErrorCode::ServerBusy,
            Self::Internal { .. } => ServiceErrorCode::InternalError,
        }
    }

    pub fn http_status_code(&self) -> u16 {
        match self {
            Self::Unauthenticated => 401,
            Self::Forbidden { .. } => 403,
            Self::TenantRequired | Self::WorkspaceRequired | Self::Validation { .. } => 400,
            Self::TenantNotFound { .. }
            | Self::WorkspaceNotFound { .. }
            | Self::NotFound { .. } => 404,
            Self::Conflict { .. } | Self::IdempotencyConflict => 409,
            Self::RateLimited => 429,
            Self::CapabilityNotAvailable { .. }
            | Self::DesktopAgentOffline
            | Self::DesktopAgentNotPaired
            | Self::ServerBusy => 503,
            Self::Internal { .. } => 500,
        }
    }

    pub fn user_safe_message(&self) -> String {
        match self {
            Self::Unauthenticated => "Authentication is required.".to_string(),
            Self::Forbidden { reason } => reason.clone().unwrap_or_else(|| {
                "You do not have permission to perform this action.".to_string()
            }),
            Self::TenantRequired => "Tenant context is required.".to_string(),
            Self::WorkspaceRequired => "Workspace context is required.".to_string(),
            Self::TenantNotFound { .. } => "Tenant was not found or is not accessible.".to_string(),
            Self::WorkspaceNotFound { .. } => {
                "Workspace was not found or is not accessible.".to_string()
            }
            Self::NotFound { resource } => {
                format!("{resource} was not found or is not accessible.")
            }
            Self::Validation { message, .. } => message.clone(),
            Self::Conflict { message } => message.clone(),
            Self::IdempotencyConflict => {
                "The idempotency key was already used with a different request.".to_string()
            }
            Self::CapabilityNotAvailable { capability } => {
                format!("Capability is not available: {capability}.")
            }
            Self::DesktopAgentOffline => "Desktop agent is offline.".to_string(),
            Self::DesktopAgentNotPaired => "Desktop agent is not paired.".to_string(),
            Self::RateLimited => "Too many requests. Try again later.".to_string(),
            Self::ServerBusy => "Server is busy. Try again later.".to_string(),
            Self::Internal { .. } => "Internal server error.".to_string(),
        }
    }

    pub fn details(&self) -> BTreeMap<String, Value> {
        let mut details = BTreeMap::new();

        match self {
            Self::Forbidden {
                reason: Some(reason),
            } => {
                details.insert("reason".to_string(), Value::String(reason.clone()));
            }
            Self::TenantNotFound {
                tenant_id: Some(tenant_id),
            } => {
                details.insert(
                    "tenant_id".to_string(),
                    Value::String(tenant_id.as_str().to_string()),
                );
            }
            Self::WorkspaceNotFound {
                workspace_id: Some(workspace_id),
            } => {
                details.insert(
                    "workspace_id".to_string(),
                    Value::String(workspace_id.as_str().to_string()),
                );
            }
            Self::NotFound { resource } => {
                details.insert("resource".to_string(), Value::String(resource.clone()));
            }
            Self::Validation {
                field: Some(field), ..
            } => {
                details.insert("field".to_string(), Value::String(field.clone()));
            }
            Self::CapabilityNotAvailable { capability } => {
                details.insert("capability".to_string(), Value::String(capability.clone()));
            }
            _ => {}
        }

        details
    }

    pub fn to_api_error(&self, request_id: &RequestId) -> ApiError {
        ApiError {
            code: self.code(),
            message: self.user_safe_message(),
            request_id: request_id.as_str().to_string(),
            details: self.details(),
        }
    }

    pub fn to_api_error_response(&self, request_id: &RequestId) -> ApiErrorResponse {
        ApiErrorResponse {
            error: self.to_api_error(request_id),
        }
    }

    pub fn into_tauri_error_message(self) -> String {
        self.user_safe_message()
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.user_safe_message())
    }
}

impl std::error::Error for ServiceError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ServiceErrorCode,
    pub message: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub error: ApiError,
}

pub fn into_tauri_result<T>(result: ServiceResult<T>) -> Result<T, String> {
    result.map_err(ServiceError::into_tauri_error_message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_has_stable_code_status_and_safe_message() {
        let error = ServiceError::field_validation("name", "Name is required.");

        assert_eq!(error.code(), ServiceErrorCode::ValidationFailed);
        assert_eq!(error.code().as_str(), "validation_failed");
        assert_eq!(error.http_status_code(), 400);
        assert_eq!(error.user_safe_message(), "Name is required.");
        assert_eq!(
            error.details().get("field"),
            Some(&Value::String("name".to_string()))
        );
    }

    #[test]
    fn not_found_error_has_stable_code_and_status() {
        let error = ServiceError::not_found("Project");

        assert_eq!(error.code(), ServiceErrorCode::NotFound);
        assert_eq!(error.http_status_code(), 404);
        assert_eq!(
            error.user_safe_message(),
            "Project was not found or is not accessible."
        );
        assert_eq!(
            error.details().get("resource"),
            Some(&Value::String("Project".to_string()))
        );
    }

    #[test]
    fn forbidden_error_maps_to_permission_denied() {
        let error = ServiceError::forbidden();

        assert_eq!(error.code(), ServiceErrorCode::PermissionDenied);
        assert_eq!(error.http_status_code(), 403);
        assert_eq!(
            error.user_safe_message(),
            "You do not have permission to perform this action."
        );
    }

    #[test]
    fn conflict_error_maps_to_http_conflict() {
        let error = ServiceError::conflict("Workspace has changed.");

        assert_eq!(error.code(), ServiceErrorCode::Conflict);
        assert_eq!(error.http_status_code(), 409);
        assert_eq!(error.user_safe_message(), "Workspace has changed.");
    }

    #[test]
    fn internal_error_does_not_leak_source_to_safe_message_or_tauri_conversion() {
        let error = ServiceError::internal("database password=secret failed");

        assert_eq!(error.code(), ServiceErrorCode::InternalError);
        assert_eq!(error.http_status_code(), 500);
        assert_eq!(error.user_safe_message(), "Internal server error.");
        assert_eq!(
            error.clone().into_tauri_error_message(),
            "Internal server error."
        );
        assert!(!error.user_safe_message().contains("password=secret"));
    }

    #[test]
    fn unavailable_error_class_maps_to_503() {
        let error = ServiceError::server_busy();

        assert_eq!(error.code(), ServiceErrorCode::ServerBusy);
        assert_eq!(error.code().as_str(), "server_busy");
        assert_eq!(error.http_status_code(), 503);
        assert_eq!(
            error.user_safe_message(),
            "Server is busy. Try again later."
        );
    }

    #[test]
    fn api_error_response_matches_contract_shape() {
        let request_id = RequestId::from_static("req_1");
        let error = ServiceError::workspace_not_found(Some(WorkspaceId::from_static("wksp_1")));
        let response = error.to_api_error_response(&request_id);
        let value = serde_json::to_value(response).expect("error response serializes");

        assert_eq!(value["error"]["code"], "workspace_not_found");
        assert_eq!(
            value["error"]["message"],
            "Workspace was not found or is not accessible."
        );
        assert_eq!(value["error"]["request_id"], "req_1");
        assert_eq!(value["error"]["details"]["workspace_id"], "wksp_1");
    }

    #[test]
    fn current_tauri_result_conversion_keeps_existing_string_boundary() {
        let result: ServiceResult<()> = Err(ServiceError::validation("Invalid request."));

        assert_eq!(
            into_tauri_result(result),
            Err("Invalid request.".to_string())
        );
    }
}
