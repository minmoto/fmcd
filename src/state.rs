use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::{FederationId, FederationIdPrefix};
use tracing::{info, warn};

use crate::error::{AppError, ErrorCategory};
use crate::events::handlers::{LoggingEventHandler, MetricsEventHandler};
use crate::events::EventBus;
use crate::multimint::MultiMint;
use crate::services::{
    BalanceMonitor, BalanceMonitorConfig, DepositMonitor, DepositMonitorConfig,
    PaymentLifecycleConfig, PaymentLifecycleManager,
};
use crate::webhooks::{WebhookConfig, WebhookNotifier};

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
#[derive(Clone)]
pub struct AppState {
    pub multimint: MultiMint,
    pub start_time: Instant,
    pub event_bus: Arc<EventBus>,
    pub deposit_monitor: Option<Arc<DepositMonitor>>,
    pub balance_monitor: Option<Arc<BalanceMonitor>>,
    pub payment_lifecycle_manager: Option<Arc<PaymentLifecycleManager>>,
}

impl AppState {
    pub async fn new(fm_db_path: PathBuf) -> Result<Self> {
        Self::new_with_config(fm_db_path, WebhookConfig::default()).await
    }

    pub async fn new_with_config(
        fm_db_path: PathBuf,
        webhook_config: WebhookConfig,
    ) -> Result<Self> {
        let clients = MultiMint::new(fm_db_path).await?;
        clients.update_gateway_caches().await?;

        // Initialize event bus with reasonable capacity
        let event_bus = Arc::new(EventBus::new(1000));

        // Register default event handlers
        let logging_handler = Arc::new(LoggingEventHandler::new(false));
        let metrics_handler = Arc::new(MetricsEventHandler::new("fmcd"));

        event_bus.register_handler(logging_handler).await;
        event_bus.register_handler(metrics_handler).await;

        // Register webhook notifier if webhooks are configured
        if webhook_config.enabled && !webhook_config.endpoints.is_empty() {
            match WebhookNotifier::new(webhook_config) {
                Ok(webhook_notifier) => {
                    let webhook_handler = Arc::new(webhook_notifier);
                    event_bus.register_handler(webhook_handler).await;
                    info!("Webhook notifier registered successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize webhook notifier: {}", e);
                }
            }
        }

        info!("Event bus initialized with all handlers");

        // Initialize monitoring services
        let deposit_monitor = Arc::new(DepositMonitor::new(
            event_bus.clone(),
            Arc::new(clients.clone()),
            DepositMonitorConfig::default(),
        ));

        let balance_monitor = Arc::new(BalanceMonitor::new(
            event_bus.clone(),
            Arc::new(clients.clone()),
            BalanceMonitorConfig::default(),
        ));

        let payment_lifecycle_manager = Arc::new(PaymentLifecycleManager::new(
            event_bus.clone(),
            Arc::new(clients.clone()),
            PaymentLifecycleConfig::default(),
        ));

        Ok(Self {
            multimint: clients,
            start_time: Instant::now(),
            event_bus,
            deposit_monitor: Some(deposit_monitor),
            balance_monitor: Some(balance_monitor),
            payment_lifecycle_manager: Some(payment_lifecycle_manager),
        })
    }

    /// Initialize AppState with custom event bus configuration and webhook
    /// config
    pub async fn new_with_event_bus_config(
        fm_db_path: PathBuf,
        event_bus_capacity: usize,
        enable_debug_logging: bool,
        webhook_config: WebhookConfig,
    ) -> Result<Self> {
        let clients = MultiMint::new(fm_db_path).await?;
        clients.update_gateway_caches().await?;

        // Initialize event bus with custom capacity
        let event_bus = Arc::new(EventBus::new(event_bus_capacity));

        // Register event handlers with custom configuration
        let logging_handler = Arc::new(LoggingEventHandler::new(enable_debug_logging));
        let metrics_handler = Arc::new(MetricsEventHandler::new("fmcd"));

        event_bus.register_handler(logging_handler).await;
        event_bus.register_handler(metrics_handler).await;

        // Register webhook notifier if webhooks are configured
        if webhook_config.enabled && !webhook_config.endpoints.is_empty() {
            match WebhookNotifier::new(webhook_config) {
                Ok(webhook_notifier) => {
                    let webhook_handler = Arc::new(webhook_notifier);
                    event_bus.register_handler(webhook_handler).await;
                    info!("Webhook notifier registered successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize webhook notifier: {}", e);
                }
            }
        }

        info!(
            event_bus_capacity = event_bus_capacity,
            enable_debug_logging = enable_debug_logging,
            "Event bus initialized with custom configuration"
        );

        // Initialize monitoring services
        let deposit_monitor = Arc::new(DepositMonitor::new(
            event_bus.clone(),
            Arc::new(clients.clone()),
            DepositMonitorConfig::default(),
        ));

        let balance_monitor = Arc::new(BalanceMonitor::new(
            event_bus.clone(),
            Arc::new(clients.clone()),
            BalanceMonitorConfig::default(),
        ));

        let payment_lifecycle_manager = Arc::new(PaymentLifecycleManager::new(
            event_bus.clone(),
            Arc::new(clients.clone()),
            PaymentLifecycleConfig::default(),
        ));

        Ok(Self {
            multimint: clients,
            start_time: Instant::now(),
            event_bus,
            deposit_monitor: Some(deposit_monitor),
            balance_monitor: Some(balance_monitor),
            payment_lifecycle_manager: Some(payment_lifecycle_manager),
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

    /// Start the monitoring services (deposit, balance, and payment lifecycle
    /// monitors)
    pub async fn start_monitoring_services(&self) -> Result<()> {
        if let Some(ref deposit_monitor) = self.deposit_monitor {
            deposit_monitor.start().await?;
            info!("Deposit monitor started successfully");
        }

        if let Some(ref balance_monitor) = self.balance_monitor {
            balance_monitor.start().await?;
            info!("Balance monitor started successfully");
        }

        if let Some(ref payment_lifecycle_manager) = self.payment_lifecycle_manager {
            payment_lifecycle_manager.start().await?;
            info!("Payment lifecycle manager started successfully");
        }

        Ok(())
    }

    /// Stop the monitoring services (deposit and balance monitors)
    pub async fn stop_monitoring_services(&self) -> Result<()> {
        if let Some(ref deposit_monitor) = self.deposit_monitor {
            deposit_monitor.stop().await?;
            info!("Deposit monitor stopped successfully");
        }

        if let Some(ref balance_monitor) = self.balance_monitor {
            balance_monitor.stop().await?;
            info!("Balance monitor stopped successfully");
        }

        Ok(())
    }
}
