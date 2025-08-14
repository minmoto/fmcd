use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::FederationId;
use fedimint_core::core::OperationId;
use fedimint_core::secp256k1::PublicKey;
use fedimint_core::Amount;
use fedimint_ln_client::{LightningClientModule, OutgoingLightningPayment, PayType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{error, info, info_span, instrument, Instrument};

use crate::error::{AppError, ErrorCategory};
use crate::observability::correlation::RequestContext;
use crate::observability::{sanitize_invoice, sanitize_preimage};
use crate::operations::PaymentTracker;
use crate::router::handlers::ln::{get_invoice, wait_for_ln_payment};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnPayRequest {
    pub payment_info: String,
    pub amount_msat: Option<Amount>,
    pub lnurl_comment: Option<String>,
    pub gateway_id: PublicKey,
    pub federation_id: FederationId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LnPayResponse {
    pub operation_id: OperationId,
    pub payment_type: PayType,
    pub contract_id: String,
    pub fee: Amount,
    pub preimage: String,
}

#[instrument(
    skip(client, state),
    fields(
        federation_id = %req.federation_id,
        amount_msat = %req.amount_msat.map(|a| a.msats).unwrap_or_default(),
        gateway_id = %req.gateway_id,
        operation_id = tracing::field::Empty,
        payment_status = "initiated",
        payment_id = tracing::field::Empty,
    )
)]
async fn _pay(
    state: &AppState,
    client: ClientHandleArc,
    req: LnPayRequest,
    context: RequestContext,
) -> Result<LnPayResponse, AppError> {
    let span = tracing::Span::current();

    // Get invoice with error context
    let bolt11 = get_invoice(&req).await.map_err(|e| {
        error!(error = ?e, "Failed to get invoice");
        AppError::validation_error(format!("Invalid payment info: {}", e))
            .with_context(context.clone())
    })?;

    // Initialize payment tracker
    let mut payment_tracker = PaymentTracker::new(
        req.federation_id,
        &bolt11,
        req.amount_msat.map(|a| a.msats).unwrap_or(0),
        state.event_bus.clone(),
        Some(context.clone()),
    );

    // Record the payment ID in the current span
    span.record("payment_id", payment_tracker.payment_id());

    info!(
        invoice = %sanitize_invoice(&bolt11),
        payment_id = %payment_tracker.payment_id(),
        "Processing lightning payment"
    );

    // Track payment initiation
    payment_tracker
        .initiate(
            bolt11.clone(),
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

            // Track failure using spawn to avoid blocking error return
            let event_bus = state.event_bus.clone();
            let payment_id = payment_tracker.payment_id().to_string();
            let federation_id = payment_tracker.federation_id().to_string();
            let correlation_id = payment_tracker.correlation_id().cloned();
            let error_msg_clone = error_msg.clone();

            tokio::spawn(async move {
                use chrono::Utc;

                use crate::events::FmcdEvent;

                let event = FmcdEvent::PaymentFailed {
                    payment_id,
                    federation_id,
                    reason: error_msg_clone,
                    correlation_id,
                    timestamp: Utc::now(),
                };
                let _ = event_bus.publish(event).await;
            });

            AppError::with_category(
                ErrorCategory::FederationError,
                "Lightning module not available",
            )
            .with_source(e)
            .with_context(context.clone())
        })?;

    // Select gateway with enhanced error handling
    let gateway = lightning_module
        .select_gateway(&req.gateway_id)
        .await
        .ok_or_else(|| {
            let error_msg = format!("Gateway {} not available", req.gateway_id);
            error!(
                gateway_id = %req.gateway_id,
                payment_id = %payment_tracker.payment_id(),
                available_gateways = ?lightning_module.list_gateways(),
                "Gateway selection failed"
            );

            // Track failure using spawn to avoid blocking error return
            let event_bus = state.event_bus.clone();
            let payment_id = payment_tracker.payment_id().to_string();
            let federation_id = payment_tracker.federation_id().to_string();
            let correlation_id = payment_tracker.correlation_id().cloned();
            let error_msg_clone = error_msg.clone();

            tokio::spawn(async move {
                use chrono::Utc;

                use crate::events::FmcdEvent;

                let event = FmcdEvent::PaymentFailed {
                    payment_id,
                    federation_id,
                    reason: error_msg_clone,
                    correlation_id,
                    timestamp: Utc::now(),
                };
                let _ = event_bus.publish(event).await;
            });

            AppError::gateway_error(error_msg).with_context(context.clone())
        })?;

    // Track gateway selection
    payment_tracker
        .gateway_selected(gateway.info.gateway_id.to_string())
        .await;

    info!(
        gateway_id = %gateway.info.gateway_id,
        payment_id = %payment_tracker.payment_id(),
        "Gateway selected successfully"
    );

    // Track payment execution start
    payment_tracker.start_execution().await;

    // Execute payment with tracking
    let OutgoingLightningPayment {
        payment_type,
        contract_id,
        fee,
    } = lightning_module
        .pay_bolt11_invoice(Some(gateway), bolt11, ())
        .await
        .map_err(|e| {
            let error_msg = format!("Payment execution failed: {}", e);
            error!(
                error = ?e,
                gateway_id = %req.gateway_id,
                payment_id = %payment_tracker.payment_id(),
                "Payment execution failed"
            );

            // Track failure using spawn to avoid blocking error return
            let event_bus = state.event_bus.clone();
            let payment_id = payment_tracker.payment_id().to_string();
            let federation_id = payment_tracker.federation_id().to_string();
            let correlation_id = payment_tracker.correlation_id().cloned();
            let error_msg_clone = error_msg.clone();

            tokio::spawn(async move {
                use chrono::Utc;

                use crate::events::FmcdEvent;

                let event = FmcdEvent::PaymentFailed {
                    payment_id,
                    federation_id,
                    reason: error_msg_clone,
                    correlation_id,
                    timestamp: Utc::now(),
                };
                let _ = event_bus.publish(event).await;
            });

            AppError::gateway_error("Payment execution failed")
                .with_source(e)
                .with_context(context.clone())
        })?;

    let operation_id = payment_type.operation_id();
    span.record("operation_id", &operation_id.to_string());
    span.record("payment_status", "executing");

    info!(
        operation_id = %operation_id,
        fee_msat = %fee.msats,
        contract_id = %contract_id,
        "Payment initiated successfully"
    );

    // Wait for payment completion
    let result = wait_for_ln_payment(&client, payment_type, contract_id.to_string(), false)
        .await?
        .ok_or_else(|| {
            let error_msg = "Payment failed or timed out".to_string();
            error!(
                operation_id = %operation_id,
                payment_id = %payment_tracker.payment_id(),
                "Payment failed or timed out"
            );
            span.record("payment_status", "failed");

            // Track failure using spawn to avoid blocking error return
            let event_bus = state.event_bus.clone();
            let payment_id = payment_tracker.payment_id().to_string();
            let federation_id = payment_tracker.federation_id().to_string();
            let correlation_id = payment_tracker.correlation_id().cloned();
            let error_msg_clone = error_msg.clone();

            tokio::spawn(async move {
                use chrono::Utc;

                use crate::events::FmcdEvent;

                let event = FmcdEvent::PaymentFailed {
                    payment_id,
                    federation_id,
                    reason: error_msg_clone,
                    correlation_id,
                    timestamp: Utc::now(),
                };
                let _ = event_bus.publish(event).await;
            });

            AppError::gateway_error("Payment failed or timed out").with_context(context.clone())
        })?;

    // Track payment success
    payment_tracker
        .succeed(result.preimage.clone(), fee.msats)
        .await;

    span.record("payment_status", "completed");
    info!(
        operation_id = %operation_id,
        payment_id = %payment_tracker.payment_id(),
        preimage = %sanitize_preimage(&result.preimage),
        "Payment completed successfully"
    );

    Ok(result)
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req = serde_json::from_value::<LnPayRequest>(v)
        .map_err(|e| AppError::validation_error(format!("Invalid request: {}", e)))?;

    let client = state.get_client(req.federation_id).await?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = RequestContext::new(None);
    let pay = _pay(&state, client, req, context).await?;
    let pay_json = json!(pay);
    Ok(pay_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<LnPayRequest>,
) -> Result<Json<LnPayResponse>, AppError> {
    let client = state.get_client(req.federation_id).await?;
    let pay = _pay(&state, client, req, context).await?;
    Ok(Json(pay))
}
