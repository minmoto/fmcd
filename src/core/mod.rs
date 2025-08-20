pub mod multimint;
pub mod operations;
pub mod services;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use fedimint_client::ClientHandleArc;
use fedimint_core::config::{FederationId, FederationIdPrefix};
use fedimint_core::core::OperationId;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::secp256k1::PublicKey;
use fedimint_core::{Amount, TieredCounts};
use fedimint_ln_client::{LightningClientModule, OutgoingLightningPayment, PayType};
use fedimint_ln_common::lightning_invoice::{Bolt11InvoiceDescription, Description};
use fedimint_mint_client::MintClientModule;
use fedimint_wallet_client::WalletClientModule;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// Use local module imports
use self::multimint::MultiMint;
use self::operations::payment::InvoiceTracker;
use self::operations::PaymentTracker;
use self::services::{
    BalanceMonitor, BalanceMonitorConfig, DepositMonitor, DepositMonitorConfig,
    PaymentLifecycleConfig, PaymentLifecycleManager,
};
use crate::error::{AppError, ErrorCategory};
use crate::events::handlers::{LoggingEventHandler, MetricsEventHandler};
use crate::events::EventBus;
use crate::observability::correlation::RequestContext;
use crate::webhooks::{WebhookConfig, WebhookNotifier};

/// Invoice creation request with essential fields
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnInvoiceRequest {
    pub amount_msat: Amount,
    pub description: String,
    pub expiry_time: Option<u64>,
    pub gateway_id: PublicKey,
    pub federation_id: FederationId,
    pub metadata: Option<serde_json::Value>,
}

/// Lightning payment request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnPayRequest {
    pub payment_info: String,
    pub amount_msat: Option<Amount>,
    pub lnurl_comment: Option<String>,
    pub gateway_id: PublicKey,
    pub federation_id: FederationId,
}

/// Lightning payment response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LnPayResponse {
    pub operation_id: OperationId,
    pub payment_type: PayType,
    pub contract_id: String,
    pub fee: Amount,
    pub preimage: String,
}

/// Invoice response with essential information
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LnInvoiceResponse {
    pub invoice_id: String,
    pub operation_id: OperationId,
    pub invoice: String,
    pub status: InvoiceStatus,
    pub settlement: Option<SettlementInfo>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Unified invoice status enum
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceStatus {
    Created,
    Pending,
    Claimed {
        amount_received_msat: u64,
        settled_at: chrono::DateTime<chrono::Utc>,
    },
    Expired {
        expired_at: chrono::DateTime<chrono::Utc>,
    },
    Canceled {
        reason: String,
        canceled_at: chrono::DateTime<chrono::Utc>,
    },
}

/// Settlement information structure
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SettlementInfo {
    pub amount_received_msat: u64,
    pub settled_at: chrono::DateTime<chrono::Utc>,
    pub preimage: Option<String>,
    pub gateway_fee_msat: Option<u64>,
}

/// Info response structure
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponse {
    pub network: String,
    pub meta: BTreeMap<String, String>,
    pub total_amount_msat: Amount,
    pub total_num_notes: usize,
    pub denominations_msat: TieredCounts,
}

/// Main entry point for library consumers
pub struct FmcdCore {
    pub multimint: MultiMint,
    pub start_time: Instant,
    pub event_bus: Arc<EventBus>,
    pub deposit_monitor: Option<Arc<DepositMonitor>>,
    pub balance_monitor: Option<Arc<BalanceMonitor>>,
    pub payment_lifecycle_manager: Option<Arc<PaymentLifecycleManager>>,
}

impl FmcdCore {
    pub async fn new(data_dir: PathBuf) -> Result<Self> {
        Self::new_with_config(data_dir, WebhookConfig::default()).await
    }

    pub async fn new_with_config(data_dir: PathBuf, webhook_config: WebhookConfig) -> Result<Self> {
        let multimint = MultiMint::new(data_dir).await?;
        multimint.update_gateway_caches().await?;

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
            Arc::new(multimint.clone()),
            DepositMonitorConfig::default(),
        ));

        let balance_monitor = Arc::new(BalanceMonitor::new(
            event_bus.clone(),
            Arc::new(multimint.clone()),
            BalanceMonitorConfig::default(),
        ));

        let payment_lifecycle_manager = Arc::new(PaymentLifecycleManager::new(
            event_bus.clone(),
            Arc::new(multimint.clone()),
            PaymentLifecycleConfig::default(),
        ));

        Ok(Self {
            multimint,
            start_time: Instant::now(),
            event_bus,
            deposit_monitor: Some(deposit_monitor),
            balance_monitor: Some(balance_monitor),
            payment_lifecycle_manager: Some(payment_lifecycle_manager),
        })
    }

    /// Get uptime since core was initialized
    pub fn uptime(&self) -> Duration {
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

    /// Get a client by federation ID
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

    /// Get a client by federation ID prefix
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

    /// Join a federation with an invite code
    pub async fn join_federation(&mut self, invite_code: InviteCode) -> Result<FederationId> {
        let federation_id = self.multimint.register_new(invite_code).await?;
        info!("Created client for federation id: {:?}", federation_id);
        Ok(federation_id)
    }

    /// Get wallet info for all federations
    pub async fn get_info(&self) -> Result<HashMap<FederationId, InfoResponse>> {
        let mut info = HashMap::new();

        for (id, client) in self.multimint.clients.lock().await.iter() {
            let mint_client = client.get_first_module::<MintClientModule>()?;
            let wallet_client = client.get_first_module::<WalletClientModule>()?;
            let mut dbtx = client.db().begin_transaction_nc().await;
            let summary = mint_client.get_note_counts_by_denomination(&mut dbtx).await;

            info.insert(
                *id,
                InfoResponse {
                    network: wallet_client.get_network().to_string(),
                    meta: client.config().await.global.meta.clone(),
                    total_amount_msat: summary.total_amount(),
                    total_num_notes: summary.count_items(),
                    denominations_msat: summary,
                },
            );
        }

        Ok(info)
    }

    /// Create a lightning invoice
    pub async fn create_invoice(
        &self,
        req: LnInvoiceRequest,
        context: RequestContext,
    ) -> Result<LnInvoiceResponse, AppError> {
        use chrono::Utc;
        use uuid::Uuid;

        let client = self.get_client(req.federation_id).await?;

        let lightning_module = client
            .get_first_module::<LightningClientModule>()
            .map_err(|e| {
                error!(
                    federation_id = %req.federation_id,
                    error = ?e,
                    "Failed to get Lightning module from fedimint client"
                );
                AppError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    anyhow!("Failed to get Lightning module: {}", e),
                )
            })?;

        let gateway = lightning_module
            .select_gateway(&req.gateway_id)
            .await
            .ok_or_else(|| {
                error!(
                    gateway_id = %req.gateway_id,
                    federation_id = %req.federation_id,
                    "Failed to select gateway - gateway may be offline or not registered"
                );
                AppError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    anyhow!("Failed to select gateway with ID {}. Gateway may be offline or not registered with this federation.", req.gateway_id),
                )
            })?;

        info!(
            gateway_id = %gateway.gateway_id,
            federation_id = %req.federation_id,
            amount_msat = %req.amount_msat.msats,
            "Creating invoice with automatic monitoring"
        );

        let created_at = Utc::now();
        let expires_at = req
            .expiry_time
            .map(|expiry| created_at + chrono::Duration::seconds(expiry as i64));

        // Use provided metadata or default to null
        let metadata = req.metadata.clone().unwrap_or(serde_json::Value::Null);

        // Create fedimint invoice using native client
        let (operation_id, invoice, _) = lightning_module
            .create_bolt11_invoice(
                req.amount_msat,
                Bolt11InvoiceDescription::Direct(
                    Description::new(req.description.clone()).map_err(|e| {
                        error!(
                            federation_id = %req.federation_id,
                            description = %req.description,
                            error = ?e,
                            "Invalid invoice description"
                        );
                        AppError::new(
                            axum::http::StatusCode::BAD_REQUEST,
                            anyhow!("Invalid invoice description: {}", e),
                        )
                    })?,
                ),
                req.expiry_time,
                metadata,
                Some(gateway),
            )
            .await
            .map_err(|e| {
                error!(
                    federation_id = %req.federation_id,
                    amount_msat = %req.amount_msat.msats,
                    error = ?e,
                    "Failed to create fedimint invoice"
                );
                AppError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    anyhow!("Failed to create invoice: {}", e),
                )
            })?;

        // Generate unique invoice ID for tracking
        let invoice_id = format!("inv_{}", Uuid::new_v4().simple());

        // Create invoice tracker for observability
        let invoice_tracker = InvoiceTracker::new(
            invoice_id.clone(),
            req.federation_id,
            self.event_bus.clone(),
            Some(context.clone()),
        );

        // Track invoice creation
        invoice_tracker
            .created(req.amount_msat.msats, invoice.to_string())
            .await;

        let response = LnInvoiceResponse {
            invoice_id: invoice_id.clone(),
            operation_id,
            invoice: invoice.to_string(),
            status: InvoiceStatus::Created,
            settlement: None,
            created_at,
            expires_at,
            metadata: req.metadata.clone(),
        };

        // Register with payment lifecycle manager for comprehensive tracking
        if let Some(ref payment_lifecycle_manager) = self.payment_lifecycle_manager {
            if let Err(e) = payment_lifecycle_manager
                .track_lightning_receive(
                    operation_id,
                    req.federation_id,
                    req.amount_msat,
                    req.metadata.clone(),
                    Some(context.correlation_id.clone()),
                )
                .await
            {
                error!(
                    operation_id = ?operation_id,
                    invoice_id = %invoice_id,
                    error = ?e,
                    "Failed to register invoice with payment lifecycle manager"
                );
            } else {
                info!(
                    operation_id = ?operation_id,
                    invoice_id = %invoice_id,
                    "Invoice registered with payment lifecycle manager for automatic ecash claiming"
                );
            }
        }

        // Start automatic monitoring for the invoice
        self.start_invoice_monitoring(
            client,
            operation_id,
            invoice_id.clone(),
            req.amount_msat.msats,
            invoice_tracker,
        )
        .await;

        info!(
            operation_id = ?operation_id,
            invoice_id = %invoice_id,
            federation_id = %req.federation_id,
            amount_msat = %req.amount_msat.msats,
            "Invoice created successfully with automatic monitoring"
        );

        Ok(response)
    }

    /// Pay a lightning invoice
    pub async fn pay_invoice(
        &self,
        req: LnPayRequest,
        context: RequestContext,
    ) -> Result<LnPayResponse, AppError> {
        use crate::observability::{sanitize_invoice, sanitize_preimage};

        let client = self.get_client(req.federation_id).await?;

        // Parse invoice - only support bolt11 in core, LNURL should be handled in API
        // layer
        use std::str::FromStr;

        use fedimint_ln_common::lightning_invoice::Bolt11Invoice;

        let bolt11 = Bolt11Invoice::from_str(req.payment_info.trim()).map_err(|e| {
            error!(error = ?e, "Failed to parse invoice");
            AppError::validation_error(format!("Invalid bolt11 invoice: {}", e))
                .with_context(context.clone())
        })?;

        // Validate invoice amount
        if bolt11.amount_milli_satoshis().is_none() {
            return Err(AppError::validation_error("Invoice must have an amount")
                .with_context(context.clone()));
        }
        if req.amount_msat.is_some() && bolt11.amount_milli_satoshis().is_some() {
            return Err(
                AppError::validation_error("Amount specified in both invoice and request")
                    .with_context(context.clone()),
            );
        }

        // Initialize payment tracker
        let mut payment_tracker = PaymentTracker::new(
            req.federation_id,
            &bolt11.to_string(),
            req.amount_msat.map(|a| a.msats).unwrap_or(0),
            self.event_bus.clone(),
            Some(context.clone()),
        );

        info!(
            invoice = %sanitize_invoice(&bolt11),
            payment_id = %payment_tracker.payment_id(),
            "Processing lightning payment"
        );

        // Track payment initiation
        payment_tracker
            .initiate(
                bolt11.to_string(),
                req.amount_msat.map(|a| a.msats).unwrap_or(0),
            )
            .await;

        // Get lightning module
        let lightning_module = client
            .get_first_module::<LightningClientModule>()
            .map_err(|e| {
                let error_msg = "Lightning module not available".to_string();
                error!(
                    error = ?e,
                    payment_id = %payment_tracker.payment_id(),
                    "Lightning module not available"
                );
                // Note: Can't update tracker in non-async error closure
                AppError::with_category(ErrorCategory::PaymentTimeout, error_msg)
                    .with_context(context.clone())
            })?;

        // Select gateway
        let gateway = lightning_module
            .select_gateway(&req.gateway_id)
            .await
            .ok_or_else(|| {
                let error_msg = format!("Gateway {} not available", req.gateway_id);
                error!(
                    gateway_id = %req.gateway_id,
                    payment_id = %payment_tracker.payment_id(),
                    "Gateway not available"
                );
                // Note: Can't update tracker in non-async error closure
                AppError::with_category(ErrorCategory::GatewayError, error_msg)
                    .with_context(context.clone())
            })?;

        // Create outgoing payment
        let OutgoingLightningPayment {
            payment_type,
            contract_id,
            fee,
        } = lightning_module
            .pay_bolt11_invoice(Some(gateway), bolt11, req.amount_msat)
            .await
            .map_err(|e| {
                let error_msg = format!("Payment failed: {}", e);
                error!(
                    error = ?e,
                    payment_id = %payment_tracker.payment_id(),
                    "Payment failed during execution"
                );
                // Note: Can't update tracker in non-async error closure
                AppError::with_category(ErrorCategory::PaymentTimeout, error_msg)
                    .with_context(context.clone())
            })?;

        // Extract the operation_id from the payment_type
        let operation_id = match &payment_type {
            PayType::Internal(op_id) => *op_id,
            PayType::Lightning(op_id) => *op_id,
        };

        // Wait for payment completion - inline the logic from wait_for_ln_payment
        use fedimint_ln_client::{InternalPayState, LnPayState};
        use futures_util::StreamExt;

        let preimage = match payment_type.clone() {
            PayType::Internal(op_id) => {
                let mut updates = lightning_module
                    .subscribe_internal_pay(op_id)
                    .await
                    .map_err(|e| {
                        let error_msg = format!("Failed to subscribe to payment: {}", e);
                        error!(error = ?e, payment_id = %payment_tracker.payment_id(), "Subscribe failed");
                        // Note: Can't update tracker in non-async error closure
                        AppError::validation_error(error_msg).with_context(context.clone())
                    })?
                    .into_stream();

                let mut payment_preimage = None;
                while let Some(update) = updates.next().await {
                    match update {
                        InternalPayState::Preimage(preimage) => {
                            payment_preimage = Some(hex::encode(preimage.0));
                            break;
                        }
                        InternalPayState::RefundSuccess {
                            out_points: _,
                            error,
                        } => {
                            let error_msg =
                                format!("Internal payment failed with refund. Error: {}", error);
                            error!(payment_id = %payment_tracker.payment_id(), "Payment refunded");
                            // Note: Can't update tracker in non-async error closure
                            return Err(AppError::validation_error(error_msg).with_context(context));
                        }
                        InternalPayState::UnexpectedError(e) => {
                            let error_msg = format!("Unexpected payment error: {}", e);
                            error!(payment_id = %payment_tracker.payment_id(), "Unexpected error");
                            // Note: Can't update tracker in non-async error closure
                            return Err(AppError::validation_error(error_msg).with_context(context));
                        }
                        _ => continue,
                    }
                }
                payment_preimage
            }
            PayType::Lightning(op_id) => {
                let mut updates = lightning_module
                    .subscribe_ln_pay(op_id)
                    .await
                    .map_err(|e| {
                        let error_msg = format!("Failed to subscribe to payment: {}", e);
                        error!(error = ?e, payment_id = %payment_tracker.payment_id(), "Subscribe failed");
                        // Note: Can't update tracker in non-async error closure
                        AppError::validation_error(error_msg).with_context(context.clone())
                    })?
                    .into_stream();

                let mut payment_preimage = None;
                while let Some(update) = updates.next().await {
                    match update {
                        LnPayState::Success { preimage } => {
                            payment_preimage = Some(hex::encode(preimage));
                            break;
                        }
                        LnPayState::Refunded { gateway_error } => {
                            let error_msg = format!("Payment refunded: {}", gateway_error);
                            error!(payment_id = %payment_tracker.payment_id(), "Payment refunded");
                            // Note: Can't update tracker in non-async error closure
                            return Err(AppError::validation_error(error_msg).with_context(context));
                        }
                        _ => continue,
                    }
                }
                payment_preimage
            }
        };

        let preimage = preimage.ok_or_else(|| {
            let error_msg = "Payment completed but no preimage returned".to_string();
            error!(
                payment_id = %payment_tracker.payment_id(),
                "Payment completed but no preimage returned"
            );
            // Note: Can't update tracker in non-async error closure
            AppError::validation_error(error_msg).with_context(context.clone())
        })?;

        // Track successful payment
        payment_tracker
            .succeed(
                preimage.clone(),
                req.amount_msat.map(|a| a.msats).unwrap_or(0),
                0,
            )
            .await;

        info!(
            payment_id = %payment_tracker.payment_id(),
            preimage = %sanitize_preimage(&preimage),
            "Payment completed successfully"
        );

        Ok(LnPayResponse {
            operation_id,
            payment_type,
            contract_id: contract_id.to_string(),
            fee,
            preimage,
        })
    }

    /// Start automatic monitoring for an invoice
    async fn start_invoice_monitoring(
        &self,
        client: ClientHandleArc,
        operation_id: OperationId,
        invoice_id: String,
        amount_msat: u64,
        invoice_tracker: InvoiceTracker,
    ) {
        let timeout = Duration::from_secs(24 * 60 * 60); // 24 hours max timeout

        tokio::spawn(async move {
            if let Err(e) = Self::monitor_invoice_settlement(
                client,
                operation_id,
                invoice_id.clone(),
                amount_msat,
                timeout,
                invoice_tracker,
            )
            .await
            {
                error!(
                    operation_id = ?operation_id,
                    invoice_id = %invoice_id,
                    error = ?e,
                    "Failed to automatically monitor invoice settlement"
                );
            }
        });
    }

    /// Monitor invoice settlement using fedimint's subscribe_ln_receive
    async fn monitor_invoice_settlement(
        client: ClientHandleArc,
        operation_id: OperationId,
        invoice_id: String,
        amount_msat: u64,
        timeout: Duration,
        invoice_tracker: InvoiceTracker,
    ) -> anyhow::Result<()> {
        use fedimint_ln_client::LnReceiveState;
        use futures_util::StreamExt;

        let lightning_module = client.get_first_module::<LightningClientModule>()?;

        let mut updates = lightning_module
            .subscribe_ln_receive(operation_id)
            .await?
            .into_stream();

        info!(
            operation_id = ?operation_id,
            invoice_id = %invoice_id,
            timeout_secs = timeout.as_secs(),
            "Started automatic invoice settlement monitoring"
        );

        let timeout_future = tokio::time::sleep(timeout);
        tokio::pin!(timeout_future);

        loop {
            tokio::select! {
                update = updates.next() => {
                    match update {
                        Some(LnReceiveState::Claimed) => {
                            info!(
                                operation_id = ?operation_id,
                                invoice_id = %invoice_id,
                                "Invoice settled - publishing event to event bus"
                            );

                            // Publish invoice paid event to event bus
                            invoice_tracker.paid(amount_msat).await;
                            break;
                        }
                        Some(LnReceiveState::Canceled { reason }) => {
                            warn!(
                                operation_id = ?operation_id,
                                invoice_id = %invoice_id,
                                reason = %reason,
                                "Invoice canceled - publishing event to event bus"
                            );

                            // Publish invoice expiration/cancellation event to event bus
                            invoice_tracker.expired().await;
                            break;
                        }
                        Some(state) => {
                            info!(
                                operation_id = ?operation_id,
                                invoice_id = %invoice_id,
                                state = ?state,
                                "Invoice status update - continuing automatic monitoring"
                            );
                            continue;
                        }
                        None => {
                            warn!(
                                operation_id = ?operation_id,
                                invoice_id = %invoice_id,
                                "Automatic monitoring stream ended unexpectedly"
                            );
                            break;
                        }
                    }
                }
                _ = &mut timeout_future => {
                    warn!(
                        operation_id = ?operation_id,
                        invoice_id = %invoice_id,
                        timeout_secs = timeout.as_secs(),
                        "Invoice settlement monitoring timed out"
                    );
                    break;
                }
            }
        }

        info!(
            operation_id = ?operation_id,
            invoice_id = %invoice_id,
            "Automatic invoice settlement monitoring completed"
        );

        Ok(())
    }
}
