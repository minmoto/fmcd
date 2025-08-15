use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::response::IntoResponse;
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use super::handlers;
use crate::auth::{AuthenticatedMessage, WebSocketAuth};
use crate::error::AppError;
use crate::state::AppState;

const JSONRPC_VERSION: &str = "2.0";
const JSONRPC_ERROR_INVALID_REQUEST: i16 = -32600;

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(auth): Extension<Arc<WebSocketAuth>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_socket(socket, state, auth).await {
            // Log the error or handle it as needed
            eprintln!("Error handling socket: {}", e);
        }
    })
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: JsonRpcMethod,
    pub params: Value,
    pub id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
    pub id: Option<u64>, // Nullable to handle auth errors and notifications
}

#[derive(Debug, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub code: i16,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub enum JsonRpcMethod {
    AdminBackup,
    AdminConfig,
    AdminDiscoverVersion,
    AdminFederationIds,
    AdminInfo,
    AdminJoin,
    AdminModule,
    AdminRestore,
    AdminListOperations,
    MintDecodeNotes,
    MintEncodeNotes,
    MintReissue,
    MintSpend,
    MintValidate,
    MintSplit,
    MintCombine,
    LnInvoice,
    LnInvoiceExternalPubkeyTweaked,
    LnAwaitInvoice,
    LnClaimExternalReceiveTweaked,
    LnPay,
    LnListGateways,
    WalletDepositAddress,
    WalletAwaitDeposit,
    WalletWithdraw,
}

async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    auth: Arc<WebSocketAuth>,
) -> Result<(), anyhow::Error> {
    while let Some(Ok(msg)) = socket.next().await {
        if let Message::Text(text) = msg {
            info!("Received WebSocket message");

            // If authentication is enabled, expect authenticated messages
            if auth.is_enabled() {
                let auth_msg = match serde_json::from_str::<AuthenticatedMessage>(&text) {
                    Ok(msg) => msg,
                    Err(err) => {
                        warn!("Failed to parse authenticated message: {}", err);
                        send_auth_error(&mut socket, "Invalid message format").await?;
                        continue;
                    }
                };

                // Verify HMAC signature
                if !auth_msg.verify(&auth) {
                    warn!("HMAC verification failed for WebSocket message");
                    send_auth_error(&mut socket, "Authentication failed").await?;
                    continue;
                }

                // Extract the JSON-RPC request from the authenticated payload
                let req = match serde_json::from_value::<JsonRpcRequest>(auth_msg.payload) {
                    Ok(request) => request,
                    Err(err) => {
                        send_err_invalid_req(&mut socket, err, &text).await?;
                        continue;
                    }
                };

                let res = match_method(req.clone(), state.clone()).await;
                let res_msg = create_json_rpc_response(res, req.id);

                // Send response as authenticated message
                let auth_response =
                    AuthenticatedMessage::new(serde_json::to_value(&res_msg)?, &auth).map_err(
                        |e| anyhow::anyhow!("Failed to create authenticated message: {}", e),
                    )?;
                let response_text = serde_json::to_string(&auth_response)?;
                socket.send(Message::Text(response_text)).await?;
            } else {
                // No authentication - handle as plain JSON-RPC
                let req = match serde_json::from_str::<JsonRpcRequest>(&text) {
                    Ok(request) => request,
                    Err(err) => {
                        send_err_invalid_req(&mut socket, err, &text).await?;
                        continue;
                    }
                };

                let res = match_method(req.clone(), state.clone()).await;
                let res_msg = create_json_rpc_response(res, req.id);
                let response_text = serde_json::to_string(&res_msg)?;
                socket.send(Message::Text(response_text)).await?;
            }
        }
    }

    Ok(())
}

fn create_json_rpc_response(res: Result<Value, AppError>, req_id: u64) -> JsonRpcResponse {
    match res {
        Ok(res) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: Some(res),
            error: None,
            id: Some(req_id),
        },
        Err(e) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: None,
            error: Some(JsonRpcError {
                code: e.category.status_code().as_u16() as i16,
                message: e.message.to_string(),
            }),
            id: Some(req_id),
        },
    }
}

async fn send_err_invalid_req(
    socket: &mut WebSocket,
    err: serde_json::Error,
    text: &str,
) -> Result<(), anyhow::Error> {
    // Try to extract the id from the request
    let id = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .and_then(|v| v.as_u64());

    let err_msg = JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        result: None,
        error: Some(JsonRpcError {
            code: JSONRPC_ERROR_INVALID_REQUEST,
            message: err.to_string(),
        }),
        id, // Will be None if ID couldn't be extracted
    };
    socket
        .send(Message::Text(serde_json::to_string(&err_msg)?))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send error response: {}", e))?;

    Ok(())
}

async fn send_auth_error(socket: &mut WebSocket, message: &str) -> Result<(), anyhow::Error> {
    let err_msg = JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        result: None,
        error: Some(JsonRpcError {
            code: -32001, // Custom authentication error code
            message: message.to_string(),
        }),
        id: None, // No ID available for auth errors as they occur before parsing
    };
    socket
        .send(Message::Text(serde_json::to_string(&err_msg)?))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send auth error response: {}", e))?;

    Ok(())
}

async fn match_method(req: JsonRpcRequest, state: AppState) -> Result<Value, AppError> {
    match req.method {
        JsonRpcMethod::AdminBackup => {
            handlers::admin::backup::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::AdminConfig => handlers::admin::config::handle_ws(state.clone()).await,
        JsonRpcMethod::AdminDiscoverVersion => {
            handlers::admin::discover_version::handle_ws(state.clone()).await
        }
        JsonRpcMethod::AdminFederationIds => {
            handlers::admin::federation_ids::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::AdminInfo => {
            handlers::admin::info::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::AdminJoin => {
            handlers::admin::join::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::AdminModule => {
            handlers::admin::module::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::AdminRestore => {
            handlers::admin::restore::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::AdminListOperations => {
            handlers::admin::list_operations::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::MintDecodeNotes => handlers::mint::decode_notes::handle_ws(req.params).await,
        JsonRpcMethod::MintEncodeNotes => handlers::mint::encode_notes::handle_ws(req.params).await,
        JsonRpcMethod::MintReissue => {
            handlers::mint::reissue::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::MintSpend => {
            handlers::mint::spend::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::MintValidate => {
            handlers::mint::validate::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::MintSplit => handlers::mint::split::handle_ws(req.params).await,
        JsonRpcMethod::MintCombine => handlers::mint::combine::handle_ws(req.params).await,
        JsonRpcMethod::LnInvoice => {
            handlers::ln::invoice::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::LnInvoiceExternalPubkeyTweaked => {
            handlers::ln::invoice_external_pubkey_tweaked::handle_ws(state.clone(), req.params)
                .await
        }
        JsonRpcMethod::LnAwaitInvoice => {
            handlers::ln::await_invoice::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::LnClaimExternalReceiveTweaked => {
            handlers::ln::claim_external_receive_tweaked::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::LnPay => handlers::ln::pay::handle_ws(state.clone(), req.params).await,
        JsonRpcMethod::LnListGateways => {
            handlers::ln::list_gateways::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::WalletDepositAddress => {
            handlers::onchain::deposit_address::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::WalletAwaitDeposit => {
            handlers::onchain::await_deposit::handle_ws(state.clone(), req.params).await
        }
        JsonRpcMethod::WalletWithdraw => {
            handlers::onchain::withdraw::handle_ws(state.clone(), req.params).await
        }
    }
}
