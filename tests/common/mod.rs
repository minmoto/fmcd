#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use fedimint_core::config::FederationId;
use fedimint_core::invite_code::InviteCode;
use fmcd::multimint::MultiMint;
use fmcd::state::AppState;
use bip39::Mnemonic;

/// Test configuration for setting up test environments
pub struct TestConfig {
    pub temp_dir: TempDir,
    pub mnemonic: Mnemonic,
    pub password: String,
}

impl TestConfig {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mnemonic = Mnemonic::generate(12).expect("Failed to generate mnemonic");
        let password = "test_password".to_string();

        TestConfig {
            temp_dir,
            mnemonic,
            password,
        }
    }

    pub fn work_dir(&self) -> PathBuf {
        self.temp_dir.path().to_path_buf()
    }
}

/// Create a test AppState with mock data
pub async fn create_test_app_state() -> AppState {
    let config = TestConfig::new();
    let multimint = MultiMint::new(config.work_dir()).await
        .expect("Failed to create MultiMint");

    AppState::new(Arc::new(multimint), config.password)
}

/// Create a mock invite code for testing
pub fn create_mock_invite_code() -> InviteCode {
    // This would need to be a valid invite code format
    // For testing purposes, we'll need to mock this appropriately
    todo!("Implement mock invite code generation")
}

/// Create a mock federation ID for testing
pub fn create_mock_federation_id() -> FederationId {
    // Generate a mock federation ID
    FederationId::dummy()
}

#[cfg(test)]
pub mod test_fixtures {
    use super::*;
    use serde_json::json;

    pub fn sample_mint_request() -> serde_json::Value {
        json!({
            "amount_msat": 1000,
            "federation_id": create_mock_federation_id()
        })
    }

    pub fn sample_lightning_invoice() -> String {
        // Mock lightning invoice for testing
        "lnbc1234567890...".to_string()
    }

    pub fn sample_bitcoin_address() -> String {
        "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string()
    }
}
