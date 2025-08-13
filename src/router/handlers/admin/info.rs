use std::collections::{BTreeMap, HashMap};

use anyhow::Error;
use axum::extract::State;
use axum::Json;
use fedimint_core::config::FederationId;
use fedimint_core::{Amount, TieredCounts};
use fedimint_mint_client::MintClientModule;
use fedimint_wallet_client::WalletClientModule;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::multimint::MultiMint;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponse {
    pub network: String,
    pub meta: BTreeMap<String, String>,
    pub total_amount_msat: Amount,
    pub total_num_notes: usize,
    pub denominations_msat: TieredCounts,
}

async fn _info(multimint: MultiMint) -> Result<HashMap<FederationId, InfoResponse>, Error> {
    let mut info = HashMap::new();

    for (id, client) in multimint.clients.lock().await.iter() {
        let mint_client = client.get_first_module::<MintClientModule>()?;
        let wallet_client = client.get_first_module::<WalletClientModule>()?;
        let mut dbtx = client.db().begin_transaction_nc().await;
        let summary = mint_client.get_note_counts_by_denomination(&mut dbtx).await;

        info.insert(
            *id,
            InfoResponse {
                network: wallet_client.get_network().to_string(),
                meta: client.config().await.global.meta.clone(),
                total_amount_msat: summary.total_amount(),
                total_num_notes: summary.count_items(),
                denominations_msat: summary,
            },
        );
    }

    Ok(info)
}

pub async fn handle_ws(state: AppState, _v: Value) -> Result<Value, AppError> {
    let info = _info(state.multimint).await?;
    let info_json = json!(info);
    Ok(info_json)
}

#[axum_macros::debug_handler]
pub async fn handle_rest(
    State(state): State<AppState>,
) -> Result<Json<HashMap<FederationId, InfoResponse>>, AppError> {
    let info = _info(state.multimint).await?;
    Ok(Json(info))
}
