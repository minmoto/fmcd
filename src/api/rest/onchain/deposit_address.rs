use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::core::{DepositAddressRequest, DepositAddressResponse};
use crate::error::AppError;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req: DepositAddressRequest = serde_json::from_value::<DepositAddressRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = RequestContext::new(None);
    let deposit = state.core.create_deposit_address(req, context).await?;
    let deposit_json = json!(deposit);
    Ok(deposit_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<DepositAddressRequest>,
) -> Result<Json<DepositAddressResponse>, AppError> {
    let deposit = state.core.create_deposit_address(req, context).await?;
    Ok(Json(deposit))
}
