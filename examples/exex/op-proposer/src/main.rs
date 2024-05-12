use std::{marker::PhantomData, path::Path, sync::Arc};

use alloy_network::{EthereumSigner, Network};
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_wallet::LocalWallet;
use config::OpProposerConfig;
use db::L2OutputDb;
use futures::Future;
use op_proposer::OpProposer;
use reth_exex::ExExContext;
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use rusqlite::Connection;

pub mod config;
pub mod db;
pub mod op_proposer;
pub mod tx_manager;

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    // NOTE: pass the config path as an arg instead of hardcoding
    let config_file = Path::new("./op_proposer.toml");
    let config = OpProposerConfig::load(Some(config_file))?;

    let l1_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .signer(EthereumSigner::from(config.proposer_private_key.parse::<LocalWallet>().unwrap()))
        .on_http(config.l1_rpc.parse().unwrap());

    // Initialize the L2Output database
    let connection = Connection::open(config.l2_output_db)?;
    let mut db: L2OutputDb = L2OutputDb::new(connection);
    db.initialize()?;

    let op_proposer = OpProposer::new(
        l1_provider,
        config.rollup_rpc,
        config.l2_output_oracle,
        config.l2_to_l1_message_passer,
    );

    Ok(op_proposer.spawn(ctx, db)?)
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
