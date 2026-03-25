#![allow(clippy::unwrap_used)]

use tempfile::TempDir;
use fmcd::state::AppState;
use fedimint_core::config::FederationId;

// Integration tests for the AppState and core functionality

#[tokio::test(flavor = "multi_thread")]
async fn test_app_state_creation() {
    let temp_dir = TempDir::new().unwrap();
    let work_dir = temp_dir.path().to_path_buf();

    let state = AppState::new(work_dir).await;
    assert!(state.is_ok(), "Should create AppState successfully");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multimint_federation_operations() {
    let temp_dir = TempDir::new().unwrap();
    let work_dir = temp_dir.path().to_path_buf();

    let state = AppState::new(work_dir).await.unwrap();

    // Test getting federation IDs when none exist
    let ids = state.multimint.ids().await;
    assert!(ids.is_empty(), "Should have no federations initially");

    // Test getting clients when none exist
    let clients = state.multimint.clients.lock().await;
    assert_eq!(clients.len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_nonexistent_client() {
    let temp_dir = TempDir::new().unwrap();
    let work_dir = temp_dir.path().to_path_buf();

    let state = AppState::new(work_dir).await.unwrap();
    let federation_id = FederationId::dummy();

    let result = state.get_client(federation_id).await;
    assert!(result.is_err(), "Should error when getting non-existent client");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_client_by_prefix() {
    let temp_dir = TempDir::new().unwrap();
    let work_dir = temp_dir.path().to_path_buf();

    let state = AppState::new(work_dir).await.unwrap();
    let federation_id = FederationId::dummy();
    let prefix = federation_id.to_prefix();

    let result = state.get_client_by_prefix(&prefix).await;
    assert!(result.is_err(), "Should error when getting client by non-existent prefix");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_update_gateway_caches() {
    let temp_dir = TempDir::new().unwrap();
    let work_dir = temp_dir.path().to_path_buf();

    let state = AppState::new(work_dir).await.unwrap();

    // Should handle empty client list gracefully
    let result = state.multimint.update_gateway_caches().await;
    assert!(result.is_ok(), "Should handle empty gateway cache update");
}
