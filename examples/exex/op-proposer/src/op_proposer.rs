use std::{cmp::Ordering, marker::PhantomData, sync::Arc};

use crate::{db::L2OutputDb, tx_manager::TxManager};
use alloy_network::Network;
use alloy_primitives::{Address, FixedBytes};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::sol;
use alloy_transport::Transport;
use eyre::eyre;
use futures::Future;
use reth_exex::{ExExContext, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_primitives::{BlockId, BlockNumberOrTag, B256};
use reth_provider::{BlockNumReader, StateProviderFactory};
use reth_tracing::tracing::info;
use serde::Deserialize;

use self::L2OutputOracle::L2OutputOracleInstance;

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

pub struct OpProposer<T, N, P>
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
    pub fn new(
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
        ctx: ExExContext<Node>,
        l2_output_db: L2OutputDb,
    ) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
        let l2_output_oracle =
            Arc::new(L2OutputOracle::new(self.l2_output_oracle, self.l1_provider.clone()));
        let mut transaction_manager = TxManager::new(l2_output_oracle.clone());

        let tx_manager_handle = transaction_manager.run();
        let op_proposer_handle =
            self.run(ctx, l2_output_db, l2_output_oracle.clone(), transaction_manager);

        // Create a future for the tx manager to run
        let early_handle_exit = async move {
            tokio::select! {
                _ = tx_manager_handle => {
                    info!("Tx Manager exited");
                }
                _ = op_proposer_handle => {
                    info!("Op Proposer exited");
                }
            }

            Ok(())
        };

        Ok(early_handle_exit)
    }

    pub fn run<Node: FullNodeComponents>(
        &self,
        mut ctx: ExExContext<Node>,
        mut l2_output_db: L2OutputDb,
        l2_output_oracle: Arc<L2OutputOracleInstance<T, Arc<P>, N>>,
        mut transaction_manager: TxManager<T, N, P>,
    ) -> impl Future<Output = eyre::Result<()>> {
        let l2_provider = ctx.provider().clone();
        let l1_provider = self.l1_provider.clone();
        let rollup_provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .on_http(self.rollup_provider.parse().unwrap()),
        );
        let l2_to_l1_message_passer = self.l2_to_l1_message_passer;

        async move {
            while let Some(notification) = ctx.notifications.recv().await {
                info!(?notification, "Received ExEx notification");

                //TODO: package this into function
                match &notification {
                    ExExNotification::ChainReorged { old, new } => {
                        let from = old
                            .blocks()
                            .iter()
                            .last()
                            .ok_or(eyre!("From block number of reorg not found"))?
                            .0;
                        let to = new
                            .blocks()
                            .iter()
                            .last()
                            .ok_or(eyre!("To block number of reorg not found"))?
                            .0;
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

                let l2_output = match target_block.cmp(&current_l2_block) {
                    Ordering::Less => {
                        // If the l2 output has already been submitted, we can skip this iteration
                        if transaction_manager
                            .pending_transactions
                            .lock()
                            .await
                            .contains(&target_block)
                        {
                            continue;
                        }

                        l2_output_db.get_l2_output(target_block)?
                    }
                    Ordering::Equal => {
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
                    }
                    Ordering::Greater => {
                        continue;
                    }
                };

                // Get the L2 Safe Head. If the target block is > the safe head. We shouldn't submit
                // the Proposal yet.
                let safe_head = get_l2_safe_head(rollup_provider.clone()).await?;

                if target_block <= safe_head {
                    transaction_manager.propose_l2_output(&l2_output_oracle, l2_output).await?;
                }
            }

            Ok(())
        }
    }
}
