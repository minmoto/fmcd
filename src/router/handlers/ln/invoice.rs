use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::FederationId;
use fedimint_core::core::OperationId;
use fedimint_core::secp256k1::PublicKey;
use fedimint_core::Amount;
use fedimint_ln_client::LightningClientModule;
use fedimint_ln_common::lightning_invoice::{Bolt11InvoiceDescription, Description};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{error, info, instrument};

use crate::error::AppError;
use crate::observability::correlation::RequestContext;
use crate::operations::payment::InvoiceTracker;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnInvoiceRequest {
    pub amount_msat: Amount,
    pub description: String,
    pub expiry_time: Option<u64>,
    pub gateway_id: PublicKey,
    pub federation_id: FederationId,
    pub extra_meta: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LnInvoiceResponse {
    pub operation_id: OperationId,
    pub invoice: String,
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
async fn _invoice(
    state: &AppState,
    client: ClientHandleArc,
    req: LnInvoiceRequest,
    context: RequestContext,
) -> Result<LnInvoiceResponse, AppError> {
    let span = tracing::Span::current();

    let lightning_module = client.get_first_module::<LightningClientModule>()?;
    let gateway = lightning_module
        .select_gateway(&req.gateway_id)
        .await
        .ok_or_else(|| {
            error!(
                gateway_id = %req.gateway_id,
                federation_id = %req.federation_id,
                "Failed to select gateway"
            );
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Failed to select gateway"),
            )
        })?;

    info!(
        gateway_id = %gateway.gateway_id,
        federation_id = %req.federation_id,
        amount_msat = %req.amount_msat.msats,
        "Creating invoice with selected gateway"
    );

    let (operation_id, invoice, _) = lightning_module
        .create_bolt11_invoice(
            req.amount_msat,
            Bolt11InvoiceDescription::Direct(Description::new(req.description)?),
            req.expiry_time,
            req.extra_meta.unwrap_or_default(),
            Some(gateway),
        )
        .await?;

    // Create invoice tracker using operation_id as invoice_id
    let invoice_tracker = InvoiceTracker::new(
        format!("{:?}", operation_id),
        req.federation_id,
        state.event_bus.clone(),
        Some(context),
    );

    // Record the operation and invoice IDs in the span
    span.record("operation_id", &format!("{:?}", operation_id));
    span.record("invoice_id", invoice_tracker.invoice_id());

    // Track invoice creation
    invoice_tracker
        .created(req.amount_msat.msats, invoice.to_string())
        .await;

    info!(
        operation_id = ?operation_id,
        invoice_id = %invoice_tracker.invoice_id(),
        federation_id = %req.federation_id,
        amount_msat = %req.amount_msat.msats,
        "Invoice created successfully"
    );

    Ok(LnInvoiceResponse {
        operation_id,
        invoice: invoice.to_string(),
    })
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let v = serde_json::from_value::<LnInvoiceRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    let client = state.get_client(v.federation_id).await?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = RequestContext::new(None);
    let invoice = _invoice(&state, client, v, context).await?;
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
    let invoice = _invoice(&state, client, req, context).await?;
    Ok(Json(invoice))
}
