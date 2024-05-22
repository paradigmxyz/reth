//! An example of an ExEx that proposes L2 outputs to L1

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_network::EthereumSigner;
use alloy_provider::{PendingTransaction, ProviderBuilder};
use alloy_signer_wallet::LocalWallet;
use config::OpProposerConfig;
use db::L2OutputDb;
use eyre::eyre;
use futures::Future;
use op_proposer::{L2OutputOracle, OpProposer};
use reth_exex::ExExContext;
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use rusqlite::Connection;
use std::{path::PathBuf, sync::Arc};
use tx_manager::TxManager;

pub mod config;
pub mod db;
pub mod op_proposer;
pub mod tx_manager;

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("op_proposer.toml");
    let config = OpProposerConfig::load(Some(config_path.as_path()))?;

    let l1_provider = Arc::new(
        ProviderBuilder::new()
            .with_recommended_fillers()
            .signer(EthereumSigner::from(config.proposer_private_key.parse::<LocalWallet>()?))
            .on_http(config.l1_rpc.parse()?),
    );

    // Initialize the L2Output database
    let connection = Connection::open(config.l2_output_db)?;
    let mut db: L2OutputDb = L2OutputDb::new(connection);
    db.initialize()?;

    // Initialize the l2 output oracle and the OpProposer
    let l2_output_oracle =
        Arc::new(L2OutputOracle::new(config.l2_output_oracle, l1_provider.clone()));

    let op_proposer =
        OpProposer::new(l1_provider, config.rollup_rpc, config.l2_to_l1_message_passer);

    // Initialize the TxManager to manage pending transactions
    let (pending_tx, pending_rx) = tokio::sync::mpsc::channel::<(u64, PendingTransaction)>(100);
    let mut transaction_manager = TxManager::new(l2_output_oracle.clone(), pending_tx);

    // Spawn the OpProposer and TxManager, proposing L2 outputs to L1
    let op_proposer_fut = async move {
        tokio::select! {
            _ = transaction_manager.run(pending_rx) => {
                return Err(eyre!("Tx Manager exited early"));
            }
            _ = op_proposer.run(ctx, db, l2_output_oracle, transaction_manager) => {
                return Err(eyre!("Op Proposer exited early"));
            }
        }
    };

    Ok(op_proposer_fut)
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("OpProposer", |ctx| async move { init_exex(ctx).await })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
