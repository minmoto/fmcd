use axum::extract::{Extension, State};
use axum::Json;
use serde_json::{json, Value};

use crate::api::LnurlResolver;
use crate::core::{LnPayRequest, LnPayResponse};
use crate::error::AppError;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req = serde_json::from_value::<LnPayRequest>(v)?;

    // Create a new context for backward compatibility
    let context = RequestContext::new(None);

    // Use the resolver pattern for payment info resolution
    let resolver = LnurlResolver::new();
    let response = state
        .core
        .pay_invoice_with_resolver(req, context, Some(&resolver))
        .await?;
    Ok(json!(response))
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<LnPayRequest>,
) -> Result<Json<LnPayResponse>, AppError> {
    // Use the resolver pattern for payment info resolution
    let resolver = LnurlResolver::new();
    let response = state
        .core
        .pay_invoice_with_resolver(req, context, Some(&resolver))
        .await?;
    Ok(Json(response))
}
