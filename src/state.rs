use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Result};
use axum::http::StatusCode;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::{FederationId, FederationIdPrefix};
use tracing::{info, warn};

use crate::error::{AppError, ErrorCategory};
use crate::multimint::MultiMint;
use crate::observability::correlation::RequestContext;

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
#[derive(Debug, Clone)]
pub struct AppState {
    pub multimint: MultiMint,
    pub start_time: Instant,
}

impl AppState {
    pub async fn new(fm_db_path: PathBuf) -> Result<Self> {
        let clients = MultiMint::new(fm_db_path).await?;
        clients.update_gateway_caches().await?;
        Ok(Self {
            multimint: clients,
            start_time: Instant::now(),
        })
    }

    // Helper function to get a specific client from the state or default
    pub async fn get_client(
        &self,
        federation_id: FederationId,
    ) -> Result<ClientHandleArc, AppError> {
        info!(
            federation_id = %federation_id,
            "Retrieving client for federation"
        );

        match self.multimint.get(&federation_id).await {
            Some(client) => {
                info!(
                    federation_id = %federation_id,
                    "Client retrieved successfully"
                );
                Ok(client)
            }
            None => {
                warn!(
                    federation_id = %federation_id,
                    "No client found for federation"
                );
                Err(AppError::with_category(
                    ErrorCategory::FederationNotFound,
                    format!("No client found for federation id: {}", federation_id),
                ))
            }
        }
    }

    pub async fn get_client_by_prefix(
        &self,
        federation_id_prefix: &FederationIdPrefix,
    ) -> Result<ClientHandleArc, AppError> {
        info!(
            federation_id_prefix = %federation_id_prefix,
            "Retrieving client for federation prefix"
        );

        let client = self.multimint.get_by_prefix(federation_id_prefix).await;

        match client {
            Some(client) => {
                info!(
                    federation_id_prefix = %federation_id_prefix,
                    "Client retrieved successfully by prefix"
                );
                Ok(client)
            }
            None => {
                warn!(
                    federation_id_prefix = %federation_id_prefix,
                    "No client found for federation prefix"
                );
                Err(AppError::with_category(
                    ErrorCategory::FederationNotFound,
                    format!(
                        "No client found for federation id prefix: {}",
                        federation_id_prefix
                    ),
                ))
            }
        }
    }

    pub fn uptime(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}
