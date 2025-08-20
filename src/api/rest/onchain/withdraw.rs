use std::str::FromStr;

use anyhow::anyhow;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use bitcoin::{Address, Amount, Txid};
use chrono::Utc;
use fedimint_client::ClientHandleArc;
use fedimint_core::config::FederationId;
use fedimint_core::BitcoinAmountOrAll;
use fedimint_wallet_client::{WalletClientModule, WithdrawState};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{error, info};

use crate::error::AppError;
use crate::events::FmcdEvent;
use crate::observability::correlation::RequestContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawRequest {
    pub address: String,
    pub amount_sat: BitcoinAmountOrAll,
    pub federation_id: FederationId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawResponse {
    pub txid: Txid,
    pub fees_sat: u64,
}

async fn _withdraw(
    state: &AppState,
    client: ClientHandleArc,
    req: WithdrawRequest,
    context: RequestContext,
) -> Result<WithdrawResponse, AppError> {
    let wallet_module = client.get_first_module::<WalletClientModule>()?;

    // Parse the address - from_str gives us Address<NetworkUnchecked>
    let address_unchecked = Address::from_str(&req.address)
        .map_err(|e| AppError::validation_error(format!("Invalid Bitcoin address: {}", e)))?;

    // TODO: Properly validate network - for now assuming valid
    let address = address_unchecked.assume_checked();
    let (amount, fees) = match req.amount_sat {
        // If the amount is "all", then we need to subtract the fees from
        // the amount we are withdrawing
        BitcoinAmountOrAll::All => {
            let balance = Amount::from_sat(client.get_balance().await.msats / 1000);
            let fees = wallet_module.get_withdraw_fees(&address, balance).await?;
            let amount = balance.checked_sub(fees.amount());
            let amount = match amount {
                Some(amount) => amount,
                None => {
                    return Err(AppError::new(
                        StatusCode::BAD_REQUEST,
                        anyhow!("Insufficient balance to pay fees"),
                    ))
                }
            };

            (amount, fees)
        }
        BitcoinAmountOrAll::Amount(amount) => (
            amount,
            wallet_module.get_withdraw_fees(&address, amount).await?,
        ),
    };
    let absolute_fees = fees.amount();

    info!("Attempting withdraw with fees: {fees:?}");

    let operation_id = wallet_module.withdraw(&address, amount, fees, ()).await?;

    // Emit withdrawal initiated event
    let withdrawal_initiated_event = FmcdEvent::WithdrawalInitiated {
        operation_id: format!("{:?}", operation_id),
        federation_id: req.federation_id.to_string(),
        address: address.to_string(),
        amount_sat: amount.to_sat(),
        fee_sat: absolute_fees.to_sat(),
        correlation_id: Some(context.correlation_id.clone()),
        timestamp: Utc::now(),
    };
    if let Err(e) = state.event_bus().publish(withdrawal_initiated_event).await {
        error!(
            operation_id = ?operation_id,
            correlation_id = %context.correlation_id,
            error = ?e,
            "Failed to publish withdrawal initiated event"
        );
    }

    info!(
        operation_id = ?operation_id,
        address = %address,
        amount_sat = amount.to_sat(),
        fee_sat = absolute_fees.to_sat(),
        "Withdrawal initiated"
    );

    // Register with payment lifecycle manager for comprehensive monitoring
    if let Some(ref payment_lifecycle_manager) = state.payment_lifecycle_manager() {
        if let Err(e) = payment_lifecycle_manager
            .track_onchain_withdraw(operation_id, req.federation_id, amount.to_sat())
            .await
        {
            error!(
                operation_id = ?operation_id,
                error = ?e,
                "Failed to register withdrawal with payment lifecycle manager"
            );
        } else {
            info!(
                operation_id = ?operation_id,
                "Withdrawal registered with payment lifecycle manager for monitoring"
            );
        }
    }

    let mut updates = wallet_module
        .subscribe_withdraw_updates(operation_id)
        .await?
        .into_stream();

    while let Some(update) = updates.next().await {
        info!("Update: {update:?}");

        match update {
            WithdrawState::Succeeded(txid) => {
                // Emit withdrawal succeeded event
                let withdrawal_succeeded_event = FmcdEvent::WithdrawalSucceeded {
                    operation_id: format!("{:?}", operation_id),
                    federation_id: req.federation_id.to_string(),
                    amount_sat: amount.to_sat(),
                    txid: txid.to_string(),
                    timestamp: Utc::now(),
                };
                if let Err(e) = state.event_bus().publish(withdrawal_succeeded_event).await {
                    error!(
                        operation_id = ?operation_id,
                        correlation_id = %context.correlation_id,
                        txid = %txid,
                        error = ?e,
                        "Failed to publish withdrawal completed event"
                    );
                }

                info!(
                    operation_id = ?operation_id,
                    txid = %txid,
                    "Withdrawal completed successfully"
                );

                return Ok(WithdrawResponse {
                    txid: txid,
                    fees_sat: absolute_fees.to_sat(),
                });
            }
            WithdrawState::Failed(e) => {
                let error_reason = format!("Withdraw failed: {:?}", e);

                // Emit withdrawal failed event
                let withdrawal_failed_event = FmcdEvent::WithdrawalFailed {
                    operation_id: format!("{:?}", operation_id),
                    federation_id: req.federation_id.to_string(),
                    reason: error_reason.clone(),
                    correlation_id: Some(context.correlation_id.clone()),
                    timestamp: Utc::now(),
                };
                if let Err(event_err) = state.event_bus().publish(withdrawal_failed_event).await {
                    error!(
                        operation_id = ?operation_id,
                        correlation_id = %context.correlation_id,
                        error = ?event_err,
                        "Failed to publish withdrawal failed event"
                    );
                }

                error!(
                    operation_id = ?operation_id,
                    error = ?e,
                    "Withdrawal failed"
                );

                return Err(AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    anyhow!("{}", error_reason),
                ));
            }
            _ => continue,
        };
    }

    // Emit withdrawal failed event for stream ending without outcome
    let error_reason = "Update stream ended without outcome".to_string();
    let withdrawal_failed_event = FmcdEvent::WithdrawalFailed {
        operation_id: format!("{:?}", operation_id),
        federation_id: req.federation_id.to_string(),
        reason: error_reason.clone(),
        correlation_id: Some(context.correlation_id.clone()),
        timestamp: Utc::now(),
    };
    if let Err(e) = state.event_bus().publish(withdrawal_failed_event).await {
        error!(
            operation_id = ?operation_id,
            correlation_id = %context.correlation_id,
            error = ?e,
            "Failed to publish withdrawal failed event for stream timeout"
        );
    }

    error!(
        operation_id = ?operation_id,
        "Update stream ended without outcome"
    );

    Err(AppError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        anyhow!("{}", error_reason),
    ))
}

pub async fn handle_ws(state: AppState, v: Value) -> Result<Value, AppError> {
    let req = serde_json::from_value::<WithdrawRequest>(v)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, anyhow!("Invalid request: {}", e)))?;
    let client = state.get_client(req.federation_id).await?;
    // TODO: WebSocket requests should get RequestContext from middleware
    let context = RequestContext::new(None);
    let withdraw = _withdraw(&state, client, req, context).await?;
    let withdraw_json = json!(withdraw);
    Ok(withdraw_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
    Extension(context): Extension<RequestContext>,
    Json(req): Json<WithdrawRequest>,
) -> Result<Json<WithdrawResponse>, AppError> {
    let client = state.get_client(req.federation_id).await?;
    let withdraw = _withdraw(&state, client, req, context).await?;
    Ok(Json(withdraw))
}
