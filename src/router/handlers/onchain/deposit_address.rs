use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::FederationId;
use fedimint_core::core::OperationId;
use fedimint_wallet_client::client_db::TweakIdx;
use fedimint_wallet_client::WalletClientModule;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, instrument};

use crate::error::AppError;
use crate::events::FmcdEvent;
use crate::observability::correlation::RequestContext;
use crate::services::deposit_monitor::DepositInfo;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositAddressRequest {
    pub federation_id: FederationId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositAddressResponse {
    // Use String for address to ensure proper serialization
    // Fedimint returns Address<NetworkUnchecked> but we convert to String for API responses
    pub address: String,
    pub operation_id: OperationId,
    pub tweak_idx: TweakIdx,
}

#[instrument(
    skip(client, state),
    fields(
        federation_id = %req.federation_id,
        operation_id = tracing::field::Empty,
        address = tracing::field::Empty,
    )
)]
async fn _deposit_address(
    client: ClientHandleArc,
    req: DepositAddressRequest,
    state: &AppState,
    context: Option<RequestContext>,
) -> Result<DepositAddressResponse, AppError> {
    let span = tracing::Span::current();

    let wallet_module = client.get_first_module::<WalletClientModule>()?;
    let (operation_id, address, tweak_idx) = wallet_module
        .allocate_deposit_address_expert_only(())
        .await?;

    // Record details in span
    span.record("operation_id", &format!("{:?}", operation_id));
    span.record("address", &address.to_string());

    // Emit deposit address generated event
    let event_bus = state.event_bus.clone();
    let federation_id = req.federation_id.to_string();
    let address_str = address.to_string();
    let operation_id_str = format!("{:?}", operation_id);
    let correlation_id = context.as_ref().map(|c| c.correlation_id.clone());

    tokio::spawn(async move {
        let event = FmcdEvent::DepositAddressGenerated {
            operation_id: operation_id_str,
            federation_id,
            address: address_str,
            correlation_id,
            timestamp: Utc::now(),
        };
        let _ = event_bus.publish(event).await;
    });

    // Register deposit with monitor for detection
    if let Some(ref deposit_monitor) = state.deposit_monitor {
        let deposit_info = DepositInfo {
            operation_id,
            federation_id: req.federation_id,
            address: address.to_string(),
            correlation_id: context.as_ref().map(|c| c.correlation_id.clone()),
            created_at: Utc::now(),
        };

        if let Err(e) = deposit_monitor.add_deposit(deposit_info).await {
            // Log error but don't fail the request - monitoring is best effort
            tracing::warn!(
                operation_id = ?operation_id,
                federation_id = %req.federation_id,
                error = ?e,
                "Failed to register deposit with monitor"
            );
        } else {
            tracing::debug!(
                operation_id = ?operation_id,
                federation_id = %req.federation_id,
                "Deposit registered with monitor"
            );
        }
    }

    // Register with payment lifecycle manager for automatic ecash claiming
    if let Some(ref payment_lifecycle_manager) = state.payment_lifecycle_manager {
        if let Err(e) = payment_lifecycle_manager
            .track_onchain_deposit(
                operation_id,
                req.federation_id,
                context.as_ref().map(|c| c.correlation_id.clone()),
            )
            .await
        {
            tracing::warn!(
                operation_id = ?operation_id,
                federation_id = %req.federation_id,
                error = ?e,
                "Failed to register deposit with payment lifecycle manager"
            );
        } else {
            tracing::info!(
                operation_id = ?operation_id,
                federation_id = %req.federation_id,
                "Deposit registered with payment lifecycle manager for automatic ecash claiming"
            );
        }
    }

    info!(
        federation_id = %req.federation_id,
        operation_id = ?operation_id,
        address = %address,
        "Deposit address generated successfully"
    );

    Ok(DepositAddressResponse {
        address: address.to_string(),
        operation_id,
        tweak_idx,
    })
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req: DepositAddressRequest = serde_json::from_value::<DepositAddressRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    let client = state.get_client(req.federation_id).await?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = Some(RequestContext::new(None));
    let deposit = _deposit_address(client, req, &state, context).await?;
    let deposit_json = json!(deposit);
    Ok(deposit_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<DepositAddressRequest>,
) -> Result<Json<DepositAddressResponse>, AppError> {
    let client = state.get_client(req.federation_id).await?;
    let deposit = _deposit_address(client, req, &state, Some(context)).await?;
    Ok(Json(deposit))
}
