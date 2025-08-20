use std::collections::HashMap;

use axum::extract::State;
use axum::Json;
use fedimint_core::config::FederationId;
use serde_json::{json, Value};

use crate::core::InfoResponse;
use crate::error::AppError;
use crate::state::AppState;

pub async fn handle_ws(state: AppState, _v: Value) -> Result<Value, AppError> {
    let info = state.core.get_info().await?;
    let info_json = json!(info);
    Ok(info_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
) -> Result<Json<HashMap<FederationId, InfoResponse>>, AppError> {
    let info = state.core.get_info().await?;
    Ok(Json(info))
}
