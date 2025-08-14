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
    skip(client),
    fields(
        federation_id = %req.federation_id,
        amount_msat = %req.amount_msat.map(|a| a.msats).unwrap_or_default(),
        gateway_id = %req.gateway_id,
        operation_id = tracing::field::Empty,
        payment_status = "initiated",
    )
)]
async fn _pay(
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

    info!(
        invoice = %sanitize_invoice(&bolt11),
        "Processing lightning payment"
    );

    // Get lightning module
    let lightning_module = client
        .get_first_module::<LightningClientModule>()
        .map_err(|e| {
            error!(error = ?e, "Lightning module not available");
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
            error!(
                gateway_id = %req.gateway_id,
                available_gateways = ?lightning_module.list_gateways(),
                "Gateway selection failed"
            );
            AppError::gateway_error(format!("Gateway {} not available", req.gateway_id))
                .with_context(context.clone())
        })?;

    info!(
        gateway_id = %gateway.info.gateway_id,
        "Gateway selected successfully"
    );

    // Execute payment with tracking
    let OutgoingLightningPayment {
        payment_type,
        contract_id,
        fee,
    } = lightning_module
        .pay_bolt11_invoice(Some(gateway), bolt11, ())
        .await
        .map_err(|e| {
            error!(
                error = ?e,
                gateway_id = %req.gateway_id,
                "Payment execution failed"
            );
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
            error!(
                operation_id = %operation_id,
                "Payment failed or timed out"
            );
            span.record("payment_status", "failed");
            AppError::gateway_error("Payment failed or timed out").with_context(context.clone())
        })?;

    span.record("payment_status", "completed");
    info!(
        operation_id = %operation_id,
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
    let pay = _pay(client, req, context).await?;
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
    let pay = _pay(client, req, context).await?;
    Ok(Json(pay))
}
