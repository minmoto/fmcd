use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use fedimint_core::invite_code::InviteCode;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::JoinFederationResponse;
use crate::error::AppError;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub invite_code: InviteCode,
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req = serde_json::from_value::<JoinRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = Some(RequestContext::new(None));
    let response = state.core.join_federation(req.invite_code, context).await?;
    let response_json = json!(response);
    Ok(response_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<JoinRequest>,
) -> Result<Json<JoinFederationResponse>, AppError> {
    let response = state
        .core
        .join_federation(req.invite_code, Some(context))
        .await?;
    Ok(Json(response))
}
