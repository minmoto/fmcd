use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tracing::{error, warn};

pub mod categories;

pub use categories::ErrorCategory;

use crate::observability::correlation::RequestContext;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub struct AppError {
    pub category: ErrorCategory,
    pub message: String,
    pub details: Option<serde_json::Value>,
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
    pub request_context: Option<RequestContext>,
}

impl AppError {
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn with_context(mut self, context: RequestContext) -> Self {
        self.request_context = Some(context);
        self
    }

    // Convenience constructors for common error types
    pub fn validation_error(message: impl Into<String>) -> Self {
        Self::with_category(ErrorCategory::ValidationError, message)
    }

    pub fn authentication_error(message: impl Into<String>) -> Self {
        Self::with_category(ErrorCategory::AuthenticationError, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::with_category(ErrorCategory::NotFound, message)
    }

    pub fn insufficient_funds(message: impl Into<String>) -> Self {
        Self::with_category(ErrorCategory::InsufficientFunds, message)
    }

    pub fn gateway_error(message: impl Into<String>) -> Self {
        Self::with_category(ErrorCategory::GatewayError, message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::with_category(ErrorCategory::InternalError, message)
    }

    // Legacy constructor for backward compatibility
    pub fn new(status: StatusCode, error: impl Into<anyhow::Error>) -> Self {
        let error = error.into();
        let category = match status {
            StatusCode::BAD_REQUEST => ErrorCategory::ValidationError,
            StatusCode::UNAUTHORIZED => ErrorCategory::AuthenticationError,
            StatusCode::FORBIDDEN => ErrorCategory::AuthorizationError,
            StatusCode::NOT_FOUND => ErrorCategory::NotFound,
            StatusCode::CONFLICT => ErrorCategory::Conflict,
            StatusCode::PAYMENT_REQUIRED => ErrorCategory::InsufficientFunds,
            StatusCode::REQUEST_TIMEOUT => ErrorCategory::PaymentTimeout,
            StatusCode::TOO_MANY_REQUESTS => ErrorCategory::RateLimited,
            StatusCode::SERVICE_UNAVAILABLE => ErrorCategory::ServiceUnavailable,
            StatusCode::GATEWAY_TIMEOUT => ErrorCategory::GatewayTimeout,
            StatusCode::BAD_GATEWAY => ErrorCategory::GatewayError,
            _ => ErrorCategory::InternalError,
        };

        Self::with_category(category, error.to_string()).with_source(error)
    }

    pub fn with_category(category: ErrorCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            details: None,
            source: None,
            request_context: None,
        }
    }
}

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.category.status_code();

        // Log error with full context
        if status.is_server_error() {
            error!(
                category = ?self.category,
                code = self.category.error_code(),
                message = %self.message,
                details = ?self.details,
                source = ?self.source,
                correlation_id = self.request_context.as_ref().map(|c| &c.correlation_id),
                request_id = self.request_context.as_ref().map(|c| &c.request_id),
                "Internal server error"
            );
        } else if status.is_client_error() {
            warn!(
                category = ?self.category,
                code = self.category.error_code(),
                message = %self.message,
                details = ?self.details,
                correlation_id = self.request_context.as_ref().map(|c| &c.correlation_id),
                request_id = self.request_context.as_ref().map(|c| &c.request_id),
                "Client error"
            );
        }

        // Return sanitized error to client
        let body = json!({
            "error": {
                "code": self.category.error_code(),
                "message": self.message,
                "details": self.details,
                "correlation_id": self.request_context.as_ref().map(|c| &c.correlation_id),
                "request_id": self.request_context.as_ref().map(|c| &c.request_id),
            }
        });

        (status, Json(body)).into_response()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category, self.message)
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e.as_ref())
    }
}

// Convert anyhow::Error to AppError (legacy compatibility)
impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self::internal_error(err.to_string()).with_source(err)
    }
}

// Convert fedimint errors to AppError
impl From<fedimint_client::ClientError> for AppError {
    fn from(err: fedimint_client::ClientError) -> Self {
        use fedimint_client::ClientError;

        let category = match &err {
            ClientError::ModuleNotFound(_) => ErrorCategory::NotFound,
            ClientError::EncodingError(_) => ErrorCategory::ValidationError,
            ClientError::NetworkError(_) => ErrorCategory::NetworkError,
            ClientError::DatabaseError(_) => ErrorCategory::DatabaseError,
            _ => ErrorCategory::InternalError,
        };

        Self::with_category(category, err.to_string())
    }
}
