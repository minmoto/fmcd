use std::time::Duration;

use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use fedimint_client::ClientHandleArc;
use fedimint_core::config::FederationId;
use fedimint_core::core::OperationId;
use fedimint_core::secp256k1::PublicKey;
use fedimint_core::Amount;
use fedimint_ln_client::{LightningClientModule, LnReceiveState};
use fedimint_ln_common::lightning_invoice::{Bolt11InvoiceDescription, Description};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::core::operations::payment::InvoiceTracker;
use crate::error::AppError;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

/// Invoice creation request with essential fields
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnInvoiceRequest {
    pub amount_msat: Amount,
    pub description: String,
    pub expiry_time: Option<u64>,
    pub gateway_id: PublicKey,
    pub federation_id: FederationId,
    /// Optional metadata to store with the invoice (e.g., order ID, customer
    /// info)
    pub metadata: Option<serde_json::Value>,
}

/// Invoice response with essential information
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LnInvoiceResponse {
    /// Unique invoice identifier for tracking
    pub invoice_id: String,
    /// Fedimint operation ID
    pub operation_id: OperationId,
    /// BOLT11 invoice string
    pub invoice: String,
    /// Current invoice status
    pub status: InvoiceStatus,
    /// Settlement information (if available)
    pub settlement: Option<SettlementInfo>,
    /// Invoice creation timestamp
    pub created_at: DateTime<Utc>,
    /// Invoice expiry timestamp
    pub expires_at: Option<DateTime<Utc>>,
    /// Optional metadata associated with the invoice
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
        settled_at: DateTime<Utc>,
    },
    Expired {
        expired_at: DateTime<Utc>,
    },
    Canceled {
        reason: String,
        canceled_at: DateTime<Utc>,
    },
}

/// Settlement information structure
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SettlementInfo {
    pub amount_received_msat: u64,
    pub settled_at: DateTime<Utc>,
    pub preimage: Option<String>,
    pub gateway_fee_msat: Option<u64>,
}

/// Invoice status update for streaming
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceStatusUpdate {
    pub invoice_id: String,
    pub operation_id: OperationId,
    pub status: InvoiceStatus,
    pub settlement: Option<SettlementInfo>,
    pub updated_at: DateTime<Utc>,
}

#[instrument(
    skip(client, state),
    fields(
        federation_id = %req.federation_id,
        amount_msat = %req.amount_msat.msats,
        gateway_id = %req.gateway_id,
        operation_id = tracing::field::Empty,
        invoice_id = tracing::field::Empty,
    )
)]
async fn _create_invoice(
    state: &AppState,
    client: ClientHandleArc,
    req: LnInvoiceRequest,
    context: RequestContext,
) -> Result<LnInvoiceResponse, AppError> {
    let span = tracing::Span::current();

    let lightning_module = client
        .get_first_module::<LightningClientModule>()
        .map_err(|e| {
            error!(
                federation_id = %req.federation_id,
                error = ?e,
                "Failed to get Lightning module from fedimint client"
            );
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
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
                StatusCode::BAD_REQUEST,
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
            Bolt11InvoiceDescription::Direct(Description::new(req.description.clone()).map_err(
                |e| {
                    error!(
                        federation_id = %req.federation_id,
                        description = %req.description,
                        error = ?e,
                        "Invalid invoice description"
                    );
                    AppError::new(
                        StatusCode::BAD_REQUEST,
                        anyhow!("Invalid invoice description: {}", e),
                    )
                },
            )?),
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
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Failed to create invoice: {}", e),
            )
        })?;

    // Generate unique invoice ID for tracking (no longer stored)
    let invoice_id = format!("inv_{}", Uuid::new_v4().simple());

    // Create invoice tracker for observability
    let invoice_tracker = InvoiceTracker::new(
        invoice_id.clone(),
        req.federation_id,
        state.event_bus().clone(),
        Some(context.clone()),
    );

    // Record telemetry
    span.record("operation_id", &format!("{:?}", operation_id));
    span.record("invoice_id", &invoice_id);

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

    // Register with payment lifecycle manager for comprehensive tracking and ecash
    // claiming
    if let Some(ref payment_lifecycle_manager) = state.payment_lifecycle_manager() {
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

    // Start automatic monitoring for all invoices (legacy compatibility - will be
    // removed)
    let client_clone = client.clone();
    let timeout = Duration::from_secs(24 * 60 * 60); // 24 hours max timeout
    let invoice_tracker_clone = invoice_tracker;
    let invoice_id_clone = invoice_id.clone();
    let amount_msat = req.amount_msat.msats;

    tokio::spawn(async move {
        if let Err(e) = monitor_invoice_settlement_automatic(
            client_clone,
            operation_id,
            invoice_id_clone.clone(),
            amount_msat,
            timeout,
            invoice_tracker_clone,
        )
        .await
        {
            error!(
                operation_id = ?operation_id,
                invoice_id = %invoice_id_clone,
                error = ?e,
                "Failed to automatically monitor invoice settlement"
            );
        }
    });

    info!(
        operation_id = ?operation_id,
        invoice_id = %invoice_id,
        federation_id = %req.federation_id,
        amount_msat = %req.amount_msat.msats,
        "Invoice created successfully with automatic monitoring"
    );

    Ok(response)
}

/// Automatic settlement monitoring using fedimint's subscribe_ln_receive
/// Monitors all invoices automatically until settled, expired, or timeout
async fn monitor_invoice_settlement_automatic(
    client: ClientHandleArc,
    operation_id: OperationId,
    invoice_id: String,
    amount_msat: u64,
    timeout: Duration,
    invoice_tracker: InvoiceTracker,
) -> anyhow::Result<()> {
    let lightning_module = client.get_first_module::<LightningClientModule>()?;

    // Use fedimint's native subscribe_ln_receive for automatic monitoring
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

                        if let Err(e) = handle_settlement_success(
                            &invoice_tracker,
                            operation_id,
                            &invoice_id,
                            amount_msat,
                        ).await {
                            error!(
                                operation_id = ?operation_id,
                                invoice_id = %invoice_id,
                                error = ?e,
                                "Failed to handle settlement success"
                            );
                        }
                        break;
                    }
                    Some(LnReceiveState::Canceled { reason }) => {
                        warn!(
                            operation_id = ?operation_id,
                            invoice_id = %invoice_id,
                            reason = %reason,
                            "Invoice canceled - publishing event to event bus"
                        );

                        if let Err(e) = handle_settlement_cancellation(
                            &invoice_tracker,
                            operation_id,
                            &invoice_id,
                            reason.to_string(),
                        ).await {
                            error!(
                                operation_id = ?operation_id,
                                invoice_id = %invoice_id,
                                error = ?e,
                                "Failed to handle settlement cancellation"
                            );
                        }
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

                if let Err(e) = handle_settlement_timeout(
                    operation_id,
                    &invoice_id,
                ).await {
                    error!(
                        operation_id = ?operation_id,
                        invoice_id = %invoice_id,
                        error = ?e,
                        "Failed to handle settlement timeout"
                    );
                }
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

async fn handle_settlement_success(
    invoice_tracker: &InvoiceTracker,
    operation_id: OperationId,
    invoice_id: &str,
    amount_msat: u64,
) -> anyhow::Result<()> {
    // NOTE: Fedimint's current API doesn't expose the actual settlement amount
    // from the LnReceiveState::Claimed state. The actual amount received might
    // differ from the invoice amount due to routing fees or other factors.
    // For now, we use the original invoice amount as a reasonable approximation.
    // This ensures webhooks and events receive meaningful data rather than 0.
    let amount_received_msat = amount_msat;

    // Publish invoice paid event to event bus
    invoice_tracker.paid(amount_received_msat).await;

    info!(
        operation_id = ?operation_id,
        invoice_id = %invoice_id,
        amount_received_msat = amount_received_msat,
        "Invoice settlement event published to event bus"
    );

    Ok(())
}

async fn handle_settlement_cancellation(
    invoice_tracker: &InvoiceTracker,
    operation_id: OperationId,
    invoice_id: &str,
    reason: String,
) -> anyhow::Result<()> {
    // Publish invoice expiration/cancellation event to event bus
    invoice_tracker.expired().await;

    info!(
        operation_id = ?operation_id,
        invoice_id = %invoice_id,
        reason = %reason,
        "Invoice cancellation event published to event bus"
    );

    Ok(())
}

async fn handle_settlement_timeout(
    operation_id: OperationId,
    invoice_id: &str,
) -> anyhow::Result<()> {
    info!(
        operation_id = ?operation_id,
        invoice_id = %invoice_id,
        "Invoice monitoring timeout - invoice may still be active in fedimint"
    );

    // Note: We don't publish an event for timeout as the invoice may still be valid
    // The timeout is just for our monitoring, not the actual invoice expiry

    Ok(())
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let v = serde_json::from_value::<LnInvoiceRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    let client = state.get_client(v.federation_id).await?;
    // Create a new context for backward compatibility (when called without context)
    let context = RequestContext::new(None);
    let invoice = _create_invoice(&state, client, v, context).await?;
    let invoice_json = json!(invoice);
    Ok(invoice_json)
}

pub async fn handle_ws_with_context(
    state: AppState,
    v: Value,
    context: RequestContext,
) -> Result<Value, AppError> {
    let v = serde_json::from_value::<LnInvoiceRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    let client = state.get_client(v.federation_id).await?;
    let invoice = _create_invoice(&state, client, v, context).await?;
    let invoice_json = json!(invoice);
    Ok(invoice_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<LnInvoiceRequest>,
) -> Result<Json<LnInvoiceResponse>, AppError> {
    let client = state.get_client(req.federation_id).await?;
    let invoice = _create_invoice(&state, client, req, context).await?;
    Ok(Json(invoice))
}
