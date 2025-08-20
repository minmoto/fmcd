use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::core::{LnInvoiceRequest, LnInvoiceResponse};
use crate::error::AppError;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req = serde_json::from_value::<LnInvoiceRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    // Create a new context for backward compatibility (when called without context)
    let context = RequestContext::new(None);
    let invoice = state.core.create_invoice(req, context).await?;
    let invoice_json = json!(invoice);
    Ok(invoice_json)
}

pub async fn handle_ws_with_context(
    state: AppState,
    v: Value,
    context: RequestContext,
) -> Result<Value, AppError> {
    let req = serde_json::from_value::<LnInvoiceRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    let invoice = state.core.create_invoice(req, context).await?;
    let invoice_json = json!(invoice);
    Ok(invoice_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<LnInvoiceRequest>,
) -> Result<Json<LnInvoiceResponse>, AppError> {
    let invoice = state.core.create_invoice(req, context).await?;
    Ok(Json(invoice))
}
