// Common types used across the library and API

// Re-export commonly used types from fedimint
pub use fedimint_core::config::FederationId;
pub use fedimint_core::core::OperationId;
pub use fedimint_core::invite_code::InviteCode;
pub use fedimint_core::Amount;
use serde::{Deserialize, Serialize};

/// Standard result type used throughout the library
pub type FmcdResult<T> = anyhow::Result<T>;

/// Common invoice structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub bolt11: String,
    pub amount_msat: u64,
    pub description: String,
    pub payment_hash: String,
    pub operation_id: OperationId,
}

/// Common payment structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payment {
    pub operation_id: OperationId,
    pub federation_id: FederationId,
    pub amount_msat: u64,
    pub status: PaymentStatus,
}

/// Payment status enumeration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaymentStatus {
    Pending,
    Success,
    Failed,
}
