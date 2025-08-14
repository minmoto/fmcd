use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Utc;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::FederationId;
use fedimint_core::core::OperationId;
use fedimint_ln_common::bitcoin::{Address, OutPoint};
use fedimint_wallet_client::{DepositStateV2, WalletClientModule};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, instrument, warn};

use crate::events::{EventBus, FmcdEvent};
use crate::multimint::MultiMint;

/// Information about an active deposit operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositInfo {
    pub operation_id: OperationId,
    pub federation_id: FederationId,
    pub address: Address,
    pub correlation_id: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
}

/// Configuration for the deposit monitor service
#[derive(Debug, Clone)]
pub struct DepositMonitorConfig {
    /// How often to poll for deposit updates (default: 30 seconds)
    pub poll_interval: Duration,
    /// Maximum number of operations to monitor simultaneously per federation
    pub max_operations_per_federation: usize,
    /// How long to monitor an operation before giving up (default: 24 hours)
    pub operation_timeout: Duration,
}

impl Default for DepositMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            max_operations_per_federation: 1000,
            operation_timeout: Duration::from_secs(24 * 60 * 60), // 24 hours
        }
    }
}

/// Service that monitors deposit addresses for incoming deposits
pub struct DepositMonitor {
    event_bus: Arc<EventBus>,
    multimint: Arc<MultiMint>,
    config: DepositMonitorConfig,
    active_deposits: Arc<RwLock<HashMap<OperationId, DepositInfo>>>,
    shutdown_tx: Arc<Mutex<Option<broadcast::Sender<()>>>>,
}

impl DepositMonitor {
    /// Create a new deposit monitor
    pub fn new(
        event_bus: Arc<EventBus>,
        multimint: Arc<MultiMint>,
        config: DepositMonitorConfig,
    ) -> Self {
        Self {
            event_bus,
            multimint,
            config,
            active_deposits: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Start the deposit monitoring service
    #[instrument(skip(self))]
    pub async fn start(&self) -> Result<()> {
        let (shutdown_tx, _) = broadcast::channel(1);
        {
            let mut tx_guard = self.shutdown_tx.lock().await;
            *tx_guard = Some(shutdown_tx.clone());
        }

        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            max_operations_per_federation = self.config.max_operations_per_federation,
            operation_timeout_hours = self.config.operation_timeout.as_secs() / 3600,
            "Starting deposit monitor service"
        );

        // Clone necessary data for the monitoring task
        let event_bus = self.event_bus.clone();
        let multimint = self.multimint.clone();
        let active_deposits = self.active_deposits.clone();
        let poll_interval = self.config.poll_interval;
        let operation_timeout = self.config.operation_timeout;

        // Spawn the monitoring task
        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_tx.subscribe();
            let mut poll_timer = interval(poll_interval);

            loop {
                tokio::select! {
                    _ = poll_timer.tick() => {
                        if let Err(e) = Self::poll_deposits(
                            &event_bus,
                            &multimint,
                            &active_deposits,
                            operation_timeout,
                        ).await {
                            error!(error = ?e, "Error during deposit polling");
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Deposit monitor received shutdown signal");
                        break;
                    }
                }
            }

            info!("Deposit monitor service stopped");
        });

        Ok(())
    }

    /// Stop the deposit monitoring service
    pub async fn stop(&self) -> Result<()> {
        let tx_guard = self.shutdown_tx.lock().await;
        if let Some(shutdown_tx) = tx_guard.as_ref() {
            let _ = shutdown_tx.send(());
        }
        Ok(())
    }

    /// Add a new deposit operation to monitor
    #[instrument(skip(self), fields(operation_id = %deposit.operation_id))]
    pub async fn add_deposit(&self, deposit: DepositInfo) -> Result<()> {
        let operation_id = deposit.operation_id;
        let federation_id = deposit.federation_id;

        // Check federation limit
        {
            let deposits = self.active_deposits.read().await;
            let federation_count = deposits
                .values()
                .filter(|d| d.federation_id == federation_id)
                .count();

            if federation_count >= self.config.max_operations_per_federation {
                warn!(
                    federation_id = %federation_id,
                    current_count = federation_count,
                    max_count = self.config.max_operations_per_federation,
                    "Federation has reached maximum deposit operations limit"
                );
                return Err(anyhow!(
                    "Federation {} has reached maximum deposit operations limit ({})",
                    federation_id,
                    self.config.max_operations_per_federation
                ));
            }
        }

        // Add to active deposits
        {
            let mut deposits = self.active_deposits.write().await;
            deposits.insert(operation_id, deposit.clone());
        }

        info!(
            operation_id = %operation_id,
            federation_id = %federation_id,
            address = %deposit.address,
            "Added deposit operation to monitor"
        );

        Ok(())
    }

    /// Remove a deposit operation from monitoring
    #[instrument(skip(self))]
    pub async fn remove_deposit(&self, operation_id: &OperationId) -> Option<DepositInfo> {
        let mut deposits = self.active_deposits.write().await;
        let removed = deposits.remove(operation_id);

        if let Some(ref deposit) = removed {
            info!(
                operation_id = %operation_id,
                federation_id = %deposit.federation_id,
                "Removed deposit operation from monitoring"
            );
        }

        removed
    }

    /// Get statistics about active deposits
    pub async fn get_stats(&self) -> DepositMonitorStats {
        let deposits = self.active_deposits.read().await;
        let mut federation_counts: HashMap<FederationId, usize> = HashMap::new();

        for deposit in deposits.values() {
            *federation_counts.entry(deposit.federation_id).or_insert(0) += 1;
        }

        DepositMonitorStats {
            total_active_deposits: deposits.len(),
            federation_counts,
        }
    }

    /// Internal method to poll all active deposits for updates
    #[instrument(skip(event_bus, multimint, active_deposits))]
    async fn poll_deposits(
        event_bus: &Arc<EventBus>,
        multimint: &Arc<MultiMint>,
        active_deposits: &Arc<RwLock<HashMap<OperationId, DepositInfo>>>,
        operation_timeout: Duration,
    ) -> Result<()> {
        let deposits_to_check = {
            let deposits = active_deposits.read().await;
            deposits.clone()
        };

        if deposits_to_check.is_empty() {
            debug!("No active deposits to monitor");
            return Ok(());
        }

        debug!(
            active_deposits = deposits_to_check.len(),
            "Polling active deposits for updates"
        );

        let now = Utc::now();
        let mut completed_operations = Vec::new();
        let mut timed_out_operations = Vec::new();

        for (operation_id, deposit_info) in &deposits_to_check {
            // Check for timeout
            if now
                .signed_duration_since(deposit_info.created_at)
                .to_std()
                .unwrap_or_default()
                > operation_timeout
            {
                timed_out_operations.push(*operation_id);
                continue;
            }

            // Get client for this federation
            let client = match multimint.get(&deposit_info.federation_id).await {
                Some(client) => client,
                None => {
                    warn!(
                        operation_id = %operation_id,
                        federation_id = %deposit_info.federation_id,
                        "Federation client not available, skipping deposit check"
                    );
                    continue;
                }
            };

            // Check deposit status
            match Self::check_deposit_status(&client, *operation_id, deposit_info).await {
                Ok(Some(deposit_result)) => {
                    // Emit deposit detected event
                    let event = FmcdEvent::DepositDetected {
                        operation_id: operation_id.to_string(),
                        federation_id: deposit_info.federation_id.to_string(),
                        address: deposit_info.address.to_string(),
                        amount_sat: deposit_result.amount_sat,
                        txid: deposit_result.txid,
                        correlation_id: deposit_info.correlation_id.clone(),
                        timestamp: Utc::now(),
                    };

                    if let Err(e) = event_bus.publish(event).await {
                        error!(
                            operation_id = %operation_id,
                            error = ?e,
                            "Failed to publish deposit detected event"
                        );
                    } else {
                        info!(
                            operation_id = %operation_id,
                            federation_id = %deposit_info.federation_id,
                            address = %deposit_info.address,
                            amount_sat = deposit_result.amount_sat,
                            txid = %deposit_result.txid,
                            "Deposit detected and event published"
                        );
                    }

                    completed_operations.push(*operation_id);
                }
                Ok(None) => {
                    // No update yet, continue monitoring
                    debug!(
                        operation_id = %operation_id,
                        "Deposit still pending"
                    );
                }
                Err(e) => {
                    error!(
                        operation_id = %operation_id,
                        error = ?e,
                        "Error checking deposit status"
                    );
                }
            }
        }

        // Remove completed and timed out operations
        {
            let mut deposits = active_deposits.write().await;
            for operation_id in completed_operations {
                deposits.remove(&operation_id);
            }
            for operation_id in timed_out_operations {
                deposits.remove(&operation_id);
                warn!(
                    operation_id = %operation_id,
                    "Deposit operation timed out and was removed from monitoring"
                );
            }
        }

        Ok(())
    }

    /// Check the status of a specific deposit operation
    async fn check_deposit_status(
        client: &ClientHandleArc,
        operation_id: OperationId,
        _deposit_info: &DepositInfo,
    ) -> Result<Option<DepositResult>> {
        let wallet_module = client
            .get_first_module::<WalletClientModule>()
            .map_err(|e| anyhow!("Failed to get wallet module: {}", e))?;

        // Subscribe to deposit updates with a timeout
        let mut updates = wallet_module
            .subscribe_deposit(operation_id)
            .await?
            .into_stream();

        // Use select to timeout the stream check quickly
        tokio::select! {
            maybe_update = updates.next() => {
                match maybe_update {
                    Some(update) => {
                        match update {
                            DepositStateV2::Confirmed { btc_deposited, btc_out_point } |
                            DepositStateV2::Claimed { btc_deposited, btc_out_point } => {
                                Ok(Some(DepositResult {
                                    amount_sat: btc_deposited.to_sat(),
                                    txid: btc_out_point.txid.to_string(),
                                    out_point: btc_out_point,
                                }))
                            }
                            DepositStateV2::Failed(reason) => {
                                warn!(
                                    operation_id = %operation_id,
                                    reason = %reason,
                                    "Deposit failed"
                                );
                                Ok(None) // Remove from monitoring
                            }
                            _ => {
                                // Still waiting (WaitingForTransaction, etc.)
                                Ok(None)
                            }
                        }
                    }
                    None => {
                        // Stream ended without result
                        warn!(
                            operation_id = %operation_id,
                            "Deposit stream ended without result"
                        );
                        Ok(None)
                    }
                }
            }
            _ = sleep(Duration::from_secs(1)) => {
                // Quick timeout - we're just checking current status, not waiting
                Ok(None)
            }
        }
    }
}

/// Result of a successful deposit detection
#[derive(Debug, Clone)]
struct DepositResult {
    amount_sat: u64,
    txid: String,
    out_point: OutPoint,
}

/// Statistics about the deposit monitor
#[derive(Debug, Clone, Serialize)]
pub struct DepositMonitorStats {
    pub total_active_deposits: usize,
    pub federation_counts: HashMap<FederationId, usize>,
}
