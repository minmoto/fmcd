pub mod multimint;
pub mod operations;
pub mod services;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bitcoin::{Address, Txid};
use fedimint_client::ClientHandleArc;
use fedimint_core::config::{FederationId, FederationIdPrefix};
use fedimint_core::core::OperationId;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::secp256k1::PublicKey;
use fedimint_core::{Amount, BitcoinAmountOrAll, TieredCounts};
use fedimint_ln_client::{LightningClientModule, OutgoingLightningPayment, PayType};
use fedimint_ln_common::lightning_invoice::{Bolt11InvoiceDescription, Description};
use fedimint_mint_client::MintClientModule;
use fedimint_wallet_client::client_db::TweakIdx;
use fedimint_wallet_client::{WalletClientModule, WithdrawState};
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

/// Trait for resolving payment information into Bolt11 invoices
/// This allows the core to remain agnostic about web protocols like LNURL
/// while allowing the API layer to provide resolution capabilities
#[async_trait::async_trait]
pub trait PaymentInfoResolver: Send + Sync {
    /// Resolve payment info (LNURL, Lightning Address, etc.) into a Bolt11
    /// invoice Returns the invoice string if resolution was successful, or
    /// None if the payment_info should be treated as a raw Bolt11 invoice
    async fn resolve_payment_info(
        &self,
        payment_info: &str,
        amount_msat: Option<Amount>,
        lnurl_comment: Option<&str>,
    ) -> Result<Option<String>, AppError>;
}

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
    /// Number of ecash notes in the mint module (does not include on-chain or
    /// lightning balances)
    pub total_num_notes: usize,
    /// Breakdown of ecash notes by denomination in the mint module
    pub denominations_msat: TieredCounts,
}

/// Join federation response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinFederationResponse {
    pub this_federation_id: FederationId,
    pub federation_ids: Vec<FederationId>,
}

/// Onchain withdraw request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawRequest {
    pub address: String,
    pub amount_sat: BitcoinAmountOrAll,
    pub federation_id: FederationId,
}

/// Onchain withdraw response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawResponse {
    pub txid: Txid,
    pub fees_sat: u64,
}

/// Deposit address request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositAddressRequest {
    pub federation_id: FederationId,
}

/// Deposit address response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositAddressResponse {
    pub address: String,
    pub operation_id: OperationId,
    pub tweak_idx: TweakIdx,
}

/// Main entry point for library consumers
pub struct FmcdCore {
    pub multimint: Arc<MultiMint>,
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
        let multimint = Arc::new(multimint);

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
            multimint.clone(),
            DepositMonitorConfig::default(),
        ));

        let balance_monitor = Arc::new(BalanceMonitor::new(
            event_bus.clone(),
            multimint.clone(),
            BalanceMonitorConfig::default(),
        ));

        let payment_lifecycle_manager = Arc::new(PaymentLifecycleManager::new(
            event_bus.clone(),
            multimint.clone(),
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
    pub async fn join_federation(
        &self,
        invite_code: InviteCode,
        context: Option<RequestContext>,
    ) -> Result<JoinFederationResponse> {
        use chrono::Utc;

        use crate::events::FmcdEvent;

        let federation_id = invite_code.federation_id();

        info!(
            federation_id = %federation_id,
            "Attempting to join federation"
        );

        // Clone multimint which is cheap due to Arc
        let mut multimint = (*self.multimint).clone();

        let this_federation_id =
            multimint
                .register_new(invite_code.clone())
                .await
                .map_err(|e| {
                    // Emit federation connection failed event
                    let event_bus = self.event_bus.clone();
                    let federation_id_str = federation_id.to_string();
                    let correlation_id = context.as_ref().map(|c| c.correlation_id.clone());
                    let error_msg = e.to_string();

                    tokio::spawn(async move {
                        let event = FmcdEvent::FederationDisconnected {
                            federation_id: federation_id_str,
                            reason: format!("Failed to join: {}", error_msg),
                            correlation_id,
                            timestamp: Utc::now(),
                        };
                        let _ = event_bus.publish(event).await;
                    });

                    e
                })?;

        // Emit federation connection success event
        let event_bus = self.event_bus.clone();
        let federation_id_str = this_federation_id.to_string();
        let correlation_id = context.as_ref().map(|c| c.correlation_id.clone());

        tokio::spawn(async move {
            let event = FmcdEvent::FederationConnected {
                federation_id: federation_id_str,
                correlation_id,
                timestamp: Utc::now(),
            };
            let _ = event_bus.publish(event).await;
        });

        // Get all federation IDs
        let federation_ids = self.multimint.ids().await.into_iter().collect::<Vec<_>>();

        info!(
            federation_id = %this_federation_id,
            total_federations = federation_ids.len(),
            "Successfully joined federation"
        );

        Ok(JoinFederationResponse {
            this_federation_id,
            federation_ids,
        })
    }

    /// Get wallet info for all federations
    pub async fn get_info(&self) -> Result<HashMap<FederationId, InfoResponse>> {
        let mut info = HashMap::new();

        for (id, client) in self.multimint.clients.lock().await.iter() {
            let mint_client = client.get_first_module::<MintClientModule>()?;
            let wallet_client = client.get_first_module::<WalletClientModule>()?;
            let mut dbtx = client.db().begin_transaction_nc().await;
            let summary = mint_client.get_note_counts_by_denomination(&mut dbtx).await;

            // Get the actual total balance from the client (includes all modules)
            let total_balance = client.get_balance().await;

            info.insert(
                *id,
                InfoResponse {
                    network: wallet_client.get_network().to_string(),
                    meta: client.config().await.global.meta.clone(),
                    total_amount_msat: total_balance,
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

    /// Generate a deposit address for receiving on-chain payments
    pub async fn create_deposit_address(
        &self,
        req: DepositAddressRequest,
        context: RequestContext,
    ) -> Result<DepositAddressResponse, AppError> {
        use chrono::Utc;

        use crate::core::services::deposit_monitor::DepositInfo;
        use crate::events::FmcdEvent;

        let client = self.get_client(req.federation_id).await?;
        let wallet_module = client
            .get_first_module::<WalletClientModule>()
            .map_err(|e| {
                error!(
                    federation_id = %req.federation_id,
                    error = ?e,
                    "Failed to get wallet module from fedimint client"
                );
                AppError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    anyhow!("Failed to get wallet module: {}", e),
                )
            })?;

        let (operation_id, address, tweak_idx) = wallet_module
            .allocate_deposit_address_expert_only(())
            .await
            .map_err(|e| {
                error!(
                    federation_id = %req.federation_id,
                    error = ?e,
                    "Failed to generate deposit address"
                );
                AppError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    anyhow!("Failed to generate deposit address: {}", e),
                )
            })?;

        // Emit deposit address generated event
        let event_bus = self.event_bus.clone();
        let federation_id_str = req.federation_id.to_string();
        let address_str = address.to_string();
        let operation_id_str = format!("{:?}", operation_id);
        let correlation_id = context.correlation_id.clone();

        tokio::spawn(async move {
            let event = FmcdEvent::DepositAddressGenerated {
                operation_id: operation_id_str,
                federation_id: federation_id_str,
                address: address_str,
                correlation_id: Some(correlation_id),
                timestamp: Utc::now(),
            };
            let _ = event_bus.publish(event).await;
        });

        // Register deposit with monitor for detection
        if let Some(ref deposit_monitor) = self.deposit_monitor {
            let deposit_info = DepositInfo {
                operation_id,
                federation_id: req.federation_id,
                address: address.to_string(),
                correlation_id: Some(context.correlation_id.clone()),
                created_at: Utc::now(),
            };

            if let Err(e) = deposit_monitor.add_deposit(deposit_info).await {
                // Log error but don't fail the request - monitoring is best effort
                warn!(
                    operation_id = ?operation_id,
                    federation_id = %req.federation_id,
                    error = ?e,
                    "Failed to register deposit with monitor"
                );
            } else {
                info!(
                    operation_id = ?operation_id,
                    federation_id = %req.federation_id,
                    "Deposit registered with monitor"
                );
            }
        }

        // Register with payment lifecycle manager for automatic ecash claiming
        if let Some(ref payment_lifecycle_manager) = self.payment_lifecycle_manager {
            if let Err(e) = payment_lifecycle_manager
                .track_onchain_deposit(
                    operation_id,
                    req.federation_id,
                    Some(context.correlation_id.clone()),
                )
                .await
            {
                warn!(
                    operation_id = ?operation_id,
                    federation_id = %req.federation_id,
                    error = ?e,
                    "Failed to register deposit with payment lifecycle manager"
                );
            } else {
                info!(
                    operation_id = ?operation_id,
                    federation_id = %req.federation_id,
                    "Deposit registered with payment lifecycle manager for automatic ecash claiming"
                );
            }
        }

        info!(
            federation_id = %req.federation_id,
            operation_id = ?operation_id,
            address = %address,
            "Deposit address generated successfully"
        );

        Ok(DepositAddressResponse {
            address: address.to_string(),
            operation_id,
            tweak_idx,
        })
    }

    /// Withdraw funds to an on-chain Bitcoin address
    pub async fn withdraw_onchain(
        &self,
        req: WithdrawRequest,
        context: RequestContext,
    ) -> Result<WithdrawResponse, AppError> {
        use std::str::FromStr;

        use chrono::Utc;
        use futures_util::StreamExt;

        use crate::events::FmcdEvent;

        let client = self.get_client(req.federation_id).await?;
        let wallet_module = client
            .get_first_module::<WalletClientModule>()
            .map_err(|e| {
                error!(
                    federation_id = %req.federation_id,
                    error = ?e,
                    "Failed to get wallet module from fedimint client"
                );
                AppError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    anyhow!("Failed to get wallet module: {}", e),
                )
            })?;

        // Parse the address - from_str gives us Address<NetworkUnchecked>
        let address_unchecked = Address::from_str(&req.address)
            .map_err(|e| AppError::validation_error(format!("Invalid Bitcoin address: {}", e)))?;

        // TODO: Properly validate network - for now assuming valid
        let address = address_unchecked.assume_checked();
        let (amount, fees) = match req.amount_sat {
            // If the amount is "all", then we need to subtract the fees from
            // the amount we are withdrawing
            BitcoinAmountOrAll::All => {
                let balance = bitcoin::Amount::from_sat(client.get_balance().await.msats / 1000);
                let fees = wallet_module.get_withdraw_fees(&address, balance).await?;
                let amount = balance.checked_sub(fees.amount());
                let amount = match amount {
                    Some(amount) => amount,
                    None => {
                        return Err(AppError::new(
                            axum::http::StatusCode::BAD_REQUEST,
                            anyhow!("Insufficient balance to pay fees"),
                        ))
                    }
                };

                (amount, fees)
            }
            BitcoinAmountOrAll::Amount(amount) => (
                amount,
                wallet_module.get_withdraw_fees(&address, amount).await?,
            ),
        };
        let absolute_fees = fees.amount();

        info!("Attempting withdraw with fees: {fees:?}");

        let operation_id = wallet_module.withdraw(&address, amount, fees, ()).await?;

        // Emit withdrawal initiated event
        let withdrawal_initiated_event = FmcdEvent::WithdrawalInitiated {
            operation_id: format!("{:?}", operation_id),
            federation_id: req.federation_id.to_string(),
            address: address.to_string(),
            amount_sat: amount.to_sat(),
            fee_sat: absolute_fees.to_sat(),
            correlation_id: Some(context.correlation_id.clone()),
            timestamp: Utc::now(),
        };
        if let Err(e) = self.event_bus.publish(withdrawal_initiated_event).await {
            error!(
                operation_id = ?operation_id,
                correlation_id = %context.correlation_id,
                error = ?e,
                "Failed to publish withdrawal initiated event"
            );
        }

        info!(
            operation_id = ?operation_id,
            address = %address,
            amount_sat = amount.to_sat(),
            fee_sat = absolute_fees.to_sat(),
            "Withdrawal initiated"
        );

        // Register with payment lifecycle manager for comprehensive monitoring
        if let Some(ref payment_lifecycle_manager) = self.payment_lifecycle_manager {
            if let Err(e) = payment_lifecycle_manager
                .track_onchain_withdraw(operation_id, req.federation_id, amount.to_sat())
                .await
            {
                error!(
                    operation_id = ?operation_id,
                    error = ?e,
                    "Failed to register withdrawal with payment lifecycle manager"
                );
            } else {
                info!(
                    operation_id = ?operation_id,
                    "Withdrawal registered with payment lifecycle manager for monitoring"
                );
            }
        }

        let mut updates = wallet_module
            .subscribe_withdraw_updates(operation_id)
            .await?
            .into_stream();

        while let Some(update) = updates.next().await {
            info!("Update: {update:?}");

            match update {
                WithdrawState::Succeeded(txid) => {
                    // Emit withdrawal succeeded event
                    let withdrawal_succeeded_event = FmcdEvent::WithdrawalSucceeded {
                        operation_id: format!("{:?}", operation_id),
                        federation_id: req.federation_id.to_string(),
                        amount_sat: amount.to_sat(),
                        txid: txid.to_string(),
                        timestamp: Utc::now(),
                    };
                    if let Err(e) = self.event_bus.publish(withdrawal_succeeded_event).await {
                        error!(
                            operation_id = ?operation_id,
                            correlation_id = %context.correlation_id,
                            txid = %txid,
                            error = ?e,
                            "Failed to publish withdrawal completed event"
                        );
                    }

                    info!(
                        operation_id = ?operation_id,
                        txid = %txid,
                        "Withdrawal completed successfully"
                    );

                    return Ok(WithdrawResponse {
                        txid: txid,
                        fees_sat: absolute_fees.to_sat(),
                    });
                }
                WithdrawState::Failed(e) => {
                    let error_reason = format!("Withdraw failed: {:?}", e);

                    // Emit withdrawal failed event
                    let withdrawal_failed_event = FmcdEvent::WithdrawalFailed {
                        operation_id: format!("{:?}", operation_id),
                        federation_id: req.federation_id.to_string(),
                        reason: error_reason.clone(),
                        correlation_id: Some(context.correlation_id.clone()),
                        timestamp: Utc::now(),
                    };
                    if let Err(event_err) = self.event_bus.publish(withdrawal_failed_event).await {
                        error!(
                            operation_id = ?operation_id,
                            correlation_id = %context.correlation_id,
                            error = ?event_err,
                            "Failed to publish withdrawal failed event"
                        );
                    }

                    error!(
                        operation_id = ?operation_id,
                        error = ?e,
                        "Withdrawal failed"
                    );

                    return Err(AppError::new(
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        anyhow!("{}", error_reason),
                    ));
                }
                _ => continue,
            };
        }

        // Emit withdrawal failed event for stream ending without outcome
        let error_reason = "Update stream ended without outcome".to_string();
        let withdrawal_failed_event = FmcdEvent::WithdrawalFailed {
            operation_id: format!("{:?}", operation_id),
            federation_id: req.federation_id.to_string(),
            reason: error_reason.clone(),
            correlation_id: Some(context.correlation_id.clone()),
            timestamp: Utc::now(),
        };
        if let Err(e) = self.event_bus.publish(withdrawal_failed_event).await {
            error!(
                operation_id = ?operation_id,
                correlation_id = %context.correlation_id,
                error = ?e,
                "Failed to publish withdrawal failed event for stream timeout"
            );
        }

        error!(
            operation_id = ?operation_id,
            "Update stream ended without outcome"
        );

        Err(AppError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            anyhow!("{}", error_reason),
        ))
    }

    /// Pay a lightning invoice
    pub async fn pay_invoice(
        &self,
        req: LnPayRequest,
        context: RequestContext,
    ) -> Result<LnPayResponse, AppError> {
        self.pay_invoice_with_resolver(req, context, None).await
    }

    /// Pay a lightning invoice with optional payment info resolver
    pub async fn pay_invoice_with_resolver(
        &self,
        mut req: LnPayRequest,
        context: RequestContext,
        resolver: Option<&dyn PaymentInfoResolver>,
    ) -> Result<LnPayResponse, AppError> {
        use crate::observability::{sanitize_invoice, sanitize_preimage};

        let client = self.get_client(req.federation_id).await?;

        // Use resolver if provided to handle non-Bolt11 payment info
        if let Some(resolver) = resolver {
            if let Some(resolved_invoice) = resolver
                .resolve_payment_info(
                    &req.payment_info,
                    req.amount_msat,
                    req.lnurl_comment.as_deref(),
                )
                .await?
            {
                req.payment_info = resolved_invoice;
            }
        }

        // Parse invoice - after resolution, this should be a bolt11 invoice
        use std::str::FromStr;

        use fedimint_ln_common::lightning_invoice::Bolt11Invoice;

        let bolt11 = Bolt11Invoice::from_str(req.payment_info.trim()).map_err(|e| {
            error!(error = ?e, "Failed to parse invoice after resolution");
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
