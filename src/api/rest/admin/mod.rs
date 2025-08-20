pub mod backup;
pub mod config;
pub mod federations;
pub mod info;
pub mod join;
pub mod module;
pub mod operations;
pub mod restore;
pub mod version;

use fedimint_client::ClientHandleArc;
use fedimint_mint_client::MintClientModule;
use fedimint_wallet_client::WalletClientModule;
use info::InfoResponse;

pub async fn _get_note_summary(client: &ClientHandleArc) -> anyhow::Result<InfoResponse> {
    let mint_client = client.get_first_module::<MintClientModule>()?;
    let wallet_client = client.get_first_module::<WalletClientModule>()?;
    let mut dbtx = client.db().begin_transaction_nc().await;
    let summary = mint_client.get_note_counts_by_denomination(&mut dbtx).await;
    Ok(InfoResponse {
        network: wallet_client.get_network().to_string(),
        meta: client.config().await.global.meta.clone(),
        total_amount_msat: summary.total_amount(),
        total_num_notes: summary.count_items(),
        denominations_msat: summary,
    })
}
