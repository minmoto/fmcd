use anyhow::{anyhow, Error};
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use fedimint_core::config::FederationId;
use fedimint_core::invite_code::InviteCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, instrument};

use crate::error::AppError;
use crate::events::FmcdEvent;
use crate::multimint::MultiMint;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub invite_code: InviteCode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinResponse {
    pub this_federation_id: FederationId,
    pub federation_ids: Vec<FederationId>,
}

#[instrument(
    skip(multimint, state),
    fields(
        federation_id = %req.invite_code.federation_id(),
        federation_id_str = tracing::field::Empty,
    )
)]
async fn _join(
    mut multimint: MultiMint,
    req: JoinRequest,
    state: &AppState,
    context: Option<RequestContext>,
) -> Result<JoinResponse, Error> {
    let span = tracing::Span::current();
    let federation_id = req.invite_code.federation_id();
    span.record("federation_id_str", &federation_id.to_string());

    info!(
        federation_id = %federation_id,
        "Joining federation"
    );

    let this_federation_id = multimint
        .register_new(req.invite_code.clone())
        .await
        .map_err(|e| {
            // Emit federation connection failed event
            let event_bus = state.event_bus.clone();
            let federation_id_str = federation_id.to_string();
            let correlation_id = context.as_ref().and_then(|c| c.correlation_id.clone());
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
    let event_bus = state.event_bus.clone();
    let federation_id_str = this_federation_id.to_string();
    let correlation_id = context.as_ref().and_then(|c| c.correlation_id.clone());
    tokio::spawn(async move {
        let event = FmcdEvent::FederationConnected {
            federation_id: federation_id_str,
            correlation_id,
            timestamp: Utc::now(),
        };
        let _ = event_bus.publish(event).await;
    });

    let federation_ids = multimint.ids().await.into_iter().collect::<Vec<_>>();

    info!(
        federation_id = %this_federation_id,
        total_federations = federation_ids.len(),
        "Successfully joined federation"
    );

    Ok(JoinResponse {
        this_federation_id,
        federation_ids,
    })
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let v = serde_json::from_value::<JoinRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = Some(RequestContext::new(None));
    let join = _join(state.multimint, v, &state, context).await?;
    let join_json = json!(join);
    Ok(join_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<JoinRequest>,
) -> Result<Json<JoinResponse>, AppError> {
    let join = _join(state.multimint, req, &state, Some(context)).await?;
    Ok(Json(join))
}
