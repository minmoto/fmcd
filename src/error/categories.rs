use std::fmt;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    // Client errors
    ValidationError,
    AuthenticationError,
    AuthorizationError,
    NotFound,
    Conflict,
    RateLimited,

    // Payment errors
    InsufficientFunds,
    PaymentTimeout,
    InvoiceExpired,
    RouteNotFound,

    // Federation errors
    FederationUnavailable,
    FederationNotFound,
    ConsensusFailure,

    // Gateway errors
    GatewayUnavailable,
    GatewayTimeout,
    GatewayError,

    // System errors
    DatabaseError,
    NetworkError,
    InternalError,
    ServiceUnavailable,
}

impl ErrorCategory {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::ValidationError => StatusCode::BAD_REQUEST,
            Self::AuthenticationError => StatusCode::UNAUTHORIZED,
            Self::AuthorizationError => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::InsufficientFunds => StatusCode::PAYMENT_REQUIRED,
            Self::PaymentTimeout | Self::InvoiceExpired => StatusCode::REQUEST_TIMEOUT,
            Self::RouteNotFound => StatusCode::NOT_FOUND,
            Self::FederationUnavailable | Self::GatewayUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::FederationNotFound => StatusCode::NOT_FOUND,
            Self::ConsensusFailure => StatusCode::INTERNAL_SERVER_ERROR,
            Self::GatewayTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::GatewayError => StatusCode::BAD_GATEWAY,
            Self::DatabaseError | Self::NetworkError | Self::InternalError => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ValidationError => "VALIDATION_ERROR",
            Self::AuthenticationError => "AUTH_FAILED",
            Self::AuthorizationError => "FORBIDDEN",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "CONFLICT",
            Self::RateLimited => "RATE_LIMITED",
            Self::InsufficientFunds => "INSUFFICIENT_FUNDS",
            Self::PaymentTimeout => "PAYMENT_TIMEOUT",
            Self::InvoiceExpired => "INVOICE_EXPIRED",
            Self::RouteNotFound => "NO_ROUTE",
            Self::FederationUnavailable => "FEDERATION_UNAVAILABLE",
            Self::FederationNotFound => "FEDERATION_NOT_FOUND",
            Self::ConsensusFailure => "CONSENSUS_FAILURE",
            Self::GatewayUnavailable => "GATEWAY_UNAVAILABLE",
            Self::GatewayTimeout => "GATEWAY_TIMEOUT",
            Self::GatewayError => "GATEWAY_ERROR",
            Self::DatabaseError => "DATABASE_ERROR",
            Self::NetworkError => "NETWORK_ERROR",
            Self::InternalError => "INTERNAL_ERROR",
            Self::ServiceUnavailable => "SERVICE_UNAVAILABLE",
        }
    }

    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            Self::ValidationError
                | Self::AuthenticationError
                | Self::AuthorizationError
                | Self::NotFound
                | Self::Conflict
                | Self::RateLimited
                | Self::InsufficientFunds
                | Self::PaymentTimeout
                | Self::InvoiceExpired
                | Self::RouteNotFound
                | Self::FederationNotFound
        )
    }

    pub fn is_server_error(&self) -> bool {
        !self.is_client_error()
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_category_status_codes() {
        assert_eq!(
            ErrorCategory::ValidationError.status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ErrorCategory::AuthenticationError.status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ErrorCategory::InsufficientFunds.status_code(),
            StatusCode::PAYMENT_REQUIRED
        );
        assert_eq!(
            ErrorCategory::InternalError.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ErrorCategory::ServiceUnavailable.status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn test_error_category_codes() {
        assert_eq!(
            ErrorCategory::ValidationError.error_code(),
            "VALIDATION_ERROR"
        );
        assert_eq!(
            ErrorCategory::AuthenticationError.error_code(),
            "AUTH_FAILED"
        );
        assert_eq!(
            ErrorCategory::InsufficientFunds.error_code(),
            "INSUFFICIENT_FUNDS"
        );
        assert_eq!(ErrorCategory::GatewayError.error_code(), "GATEWAY_ERROR");
    }

    #[test]
    fn test_client_vs_server_errors() {
        assert!(ErrorCategory::ValidationError.is_client_error());
        assert!(!ErrorCategory::ValidationError.is_server_error());

        assert!(ErrorCategory::InternalError.is_server_error());
        assert!(!ErrorCategory::InternalError.is_client_error());

        assert!(ErrorCategory::AuthenticationError.is_client_error());
        assert!(ErrorCategory::GatewayError.is_server_error());
    }

    #[test]
    fn test_error_category_display() {
        assert_eq!(
            format!("{}", ErrorCategory::ValidationError),
            "VALIDATION_ERROR"
        );
        assert_eq!(
            format!("{}", ErrorCategory::InternalError),
            "INTERNAL_ERROR"
        );
    }
}
