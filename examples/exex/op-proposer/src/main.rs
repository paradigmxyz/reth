use std::{marker::PhantomData, path::Path, sync::Arc};

use alloy_network::{EthereumSigner, Network};
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_wallet::LocalWallet;
use alloy_sol_types::sol;
use alloy_transport::Transport;
use config::OpProposerConfig;
use db::L2OutputDb;
use eyre::eyre;
use futures::Future;
use reth_exex::{ExExContext, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{BlockId, BlockNumberOrTag, B256};
use reth_provider::{BlockNumReader, StateProviderFactory};
use reth_tracing::tracing::info;
use rusqlite::Connection;
use serde::Deserialize;
pub mod config;
pub mod db;

sol! {
    #[sol(rpc)]
    contract L2OutputOracle {
        function proposeL2Output(bytes32 _outputRoot, uint256 _l2BlockNumber, bytes32 _l1BlockHash, uint256 _l1BlockNumber) external payable;
        function nextBlockNumber() public view returns (uint256);
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct L2Output {
    pub output_root: B256,
    pub l2_block_number: u64,
    pub l1_block_hash: B256,
    pub l1_block_number: u64,
}

#[derive(Deserialize, Debug)]
pub struct OptimismSyncStatus {
    pub safe_l2: L2BlockInfo,
}

#[derive(Deserialize, Debug)]
pub struct L2BlockInfo {
    pub number: u64,
}

#[derive(Deserialize, Debug)]
pub struct L1BlockAttributes {
    hash: FixedBytes<32>,
    number: u64,
}

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    // NOTE: pass the config path as an arg instead of hardcoding
    let config_file = Path::new("./op_proposer.toml");
    let config = OpProposerConfig::load(Some(config_file))?;

    let l1_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .signer(EthereumSigner::from(config.proposer_private_key.parse::<LocalWallet>().unwrap()))
        .on_http(config.l1_rpc.parse().unwrap());
    let mut db = L2OutputDb::new(connection);
    db.initialize()?;
    Ok(OpProposer::new(
        l1_provider,
        config.rollup_rpc,
        config.l2_output_oracle,
        config.l2_to_l1_message_passer,
    )
    .spawn(ctx, db)?)
}

async fn get_l1_block_attributes<T, N, P>(provider: &P) -> eyre::Result<L1BlockAttributes>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    let l1_block = provider
        .get_block(BlockId::from(BlockNumberOrTag::Latest), false)
        .await?
        .ok_or(eyre!("L1 block not found"))?;

    let l1_block_hash = l1_block.header.hash.ok_or(eyre!("L1 Block hash not found"))?;
    let l1_block_number = l1_block.header.number.ok_or(eyre!("L1 block number not found"))?;

    Ok(L1BlockAttributes { hash: l1_block_hash, number: l1_block_number })
}

async fn get_l2_safe_head<T, N, P>(provider: Arc<P>) -> eyre::Result<u64>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    let sync_status: OptimismSyncStatus =
        provider.client().request("optimism_syncStatus", ()).await?;
    let safe_head = sync_status.safe_l2.number;
    Ok(safe_head)
}

struct OpProposer<T, N, P>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    l1_provider: Arc<P>,
    rollup_provider: String,
    l2_output_oracle: Address,
    l2_to_l1_message_passer: Address,
    _pd: PhantomData<fn() -> (T, N)>,
}

impl<T: Transport + Clone, N: Network, P: Provider<T, N>> OpProposer<T, N, P> {
    fn new(
        l1_provider: P,
        rollup_provider: String,
        l2_output_oracle: Address,
        l2_to_l1_message_passer: Address,
    ) -> Self {
        Self {
            l1_provider: Arc::new(l1_provider),
            rollup_provider,
            l2_output_oracle,
            l2_to_l1_message_passer,
            _pd: PhantomData,
        }
    }

    pub fn spawn<Node: FullNodeComponents>(
        &self,
        mut ctx: ExExContext<Node>,
        mut l2_output_db: L2OutputDb,
    ) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
        let l2_output_oracle = L2OutputOracle::new(self.l2_output_oracle, self.l1_provider.clone());
        let l2_provider = ctx.provider().clone();
        let l1_provider = self.l1_provider.clone();
        let rollup_provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .on_http(self.rollup_provider.parse().unwrap()),
        );
        let l2_to_l1_message_passer = self.l2_to_l1_message_passer.clone();

        let fut = async move {
            while let Some(notification) = ctx.notifications.recv().await {
                info!(?notification, "Received ExEx notification");

                //TODO: package this into function
                match &notification {
                    ExExNotification::ChainReorged { old, new } => {
                        let from = old.blocks().iter().last().expect("TODO: Can we expect here?").0;
                        let to = new.blocks().iter().last().expect("TODO: Can we expect here?").0;
                        for block_number in *from..=*to {
                            l2_output_db.delete_l2_output(block_number)?;
                        }
                    }
                    ExExNotification::ChainReverted { old: _ } => {
                        // TODO:
                    }
                    _ => {}
                };

                let target_block = l2_output_oracle.nextBlockNumber().call().await?._0.to::<u64>();
                let current_l2_block = l2_provider.last_block_number()?;

                // Get the l2 output data to prepare for submission
                let l2_output = if target_block < current_l2_block {
                    //TODO: check the "transaction manager" to see if the proof has already
                    l2_output_db.get_l2_output(target_block)?
                } else if target_block == current_l2_block {
                    let l1_block_attr = get_l1_block_attributes(&l1_provider.clone()).await?;
                    let proof = l2_provider.latest()?.proof(l2_to_l1_message_passer, &[])?;

                    let l2_output = L2Output {
                        output_root: proof.storage_root,
                        l2_block_number: target_block,
                        l1_block_hash: l1_block_attr.hash,
                        l1_block_number: l1_block_attr.number,
                    };

                    l2_output_db.insert_l2_output(l2_output.clone())?;

                    l2_output
                } else {
                    continue;
                };

                // Get the L2 Safe Head. If the target block is < the safe head. We shouldn't submit
                // the Proposal.
                let safe_head = get_l2_safe_head(rollup_provider.clone()).await?;
                if target_block <= safe_head {
                    // Submit a transaction to propose the L2Output to the L2OutputOracle contract
                    //TODO: transaction management
                    let _: <N as Network>::ReceiptResponse = l2_output_oracle
                        .proposeL2Output(
                            l2_output.output_root,
                            U256::from(target_block),
                            l2_output.l1_block_hash,
                            U256::from(l2_output.l1_block_number),
                        )
                        .send()
                        .await?
                        .get_receipt()
                        .await?;
                    info!(
                        output_root = ?l2_output.output_root,
                        l2_block_number = ?current_l2_block,
                        l1_block_hash = ?l2_output.l1_block_hash,
                        l1_block_number = ?l2_output.l1_block_number,
                        "Successfully Proposed L2Output"
                    );
                }

                // // TODO: Wait for transaction inclusion, and add retries
            }

            Ok(())
        };

        Ok(fut)
    }
}

fn main() -> eyre::Result<()> {
    //TODO: use config crate
    //TODO: specify the db path in the config
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("OpProposer", |ctx| async move {
                let connection = Connection::open("l2_outputs.db")?;
                init_exex(ctx, connection).await
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
