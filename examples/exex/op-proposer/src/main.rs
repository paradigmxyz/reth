use std::{marker::PhantomData, sync::Arc};

use alloy_network::{EthereumSigner, Network};
use alloy_primitives::{Address, FixedBytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_wallet::LocalWallet;
use alloy_sol_types::sol;
use alloy_transport::Transport;
use eyre::eyre;
use futures::Future;
use reth_exex::{ExExContext, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{BlockId, BlockNumberOrTag};
use reth_provider::{BlockNumReader, StateProviderFactory};
use reth_tracing::tracing::info;
use rusqlite::Connection;
use serde::Deserialize;

sol! {
    #[sol(rpc)]
    contract L2OutputOracle {
        function proposeL2Output(bytes32 _outputRoot, uint256 _l2BlockNumber, bytes32 _l1BlockHash, uint256 _l1BlockNumber) external payable;
        function nextBlockNumber() public view returns (uint256);
    }
}
#[derive(Deserialize, Debug)]
pub struct OptimismSyncStatus {
    pub safe_l2: L2BlockInfo,
}

#[derive(Deserialize, Debug)]
pub struct L2BlockInfo {
    pub hash: FixedBytes<32>,
    pub number: u64,
    pub parent_hash: FixedBytes<32>,
    pub timestamp: u64,
    pub l1_origin: BlockAttributes,
}

#[derive(Deserialize, Debug)]

pub struct BlockAttributes {
    hash: FixedBytes<32>,
    number: u64,
}

/// Create SQLite tables if they do not exist.
fn create_tables(connection: &mut Connection) -> rusqlite::Result<()> {
    // Create tables to store L2 outputs
    connection.execute(
        r#"
            CREATE TABLE IF NOT EXISTS l2Outputs (
                l2_block_number     INTEGER NOT NULL PRIMARY KEY, //TODO: decide on the pk
                output_root            TEXT NOT,
                l1_block_hash          TEXT NOT NULL UNIQUE,
                l1_block_number INTEGER NOT NULL, //TODO: add a way to index
            );
            "#,
        (),
    )?;
    info!("Initialized l2Outputs table");

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct ProposerConfig {
    l1_rpc: String,
    rollup_rpc: String,
    l2_output_oracle: Address,
    l2_to_l1_message_passer: Address,
    proposer_private_key: String,
}

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    mut connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    create_tables(&mut connection)?;
    let config = serde_json::from_str::<ProposerConfig>(
        &std::fs::read_to_string("config.json").expect("Could not read config file"),
    )
    .unwrap();

    let l1_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .signer(EthereumSigner::from(config.proposer_private_key.parse::<LocalWallet>().unwrap()))
        .on_http(config.l1_rpc.parse().unwrap());

    Ok(OpProposer::new(
        l1_provider,
        config.rollup_rpc,
        config.l2_output_oracle,
        config.l2_to_l1_message_passer,
    )
    .spawn(ctx, connection)?)
}

async fn get_l1_block_attributes<T, N, P>(provider: Arc<P>) -> eyre::Result<BlockAttributes>
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

    Ok(BlockAttributes { hash: l1_block_hash, number: l1_block_number })
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

async fn get_l2_oo_data() {}

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

    fn spawn<Node: FullNodeComponents>(
        &self,
        mut ctx: ExExContext<Node>,
        _connection: Connection,
    ) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
        // TODO: add some logging for submission interval

        let l2_oo = L2OutputOracle::new(self.l2_output_oracle, self.l1_provider.clone());
        let l2_provider = ctx.provider().clone(); //TODO: update this to not clone
        let l1_provider = self.l1_provider.clone();
        let l2_to_l1_message_passer = self.l2_to_l1_message_passer.clone();
        let rollup_provider =
            Arc::new(ProviderBuilder::new().on_http(self.rollup_provider.parse().unwrap()));
        //NOTE: prune data older than some config or just keep everything, think about how much
        // data

        let fut = async move {
            while let Some(notification) = ctx.notifications.recv().await {
                match &notification {
                    ExExNotification::ChainCommitted { new } => {
                        let target_block = l2_oo.nextBlockNumber().call().await?._0.to::<u64>();
                        let current_l2_block = l2_provider.last_block_number()?;

                        //NOTE: also need to check if the tx has already been sent but not yet
                        // included
                        //TODO: bind the proof data here

                        if target_block < current_l2_block {

                            //TODO: check the "transaction manager" to see if the proof has already
                            // been submitted but not yet included
                            //TODO: get the proof data from the db
                        } else if target_block == current_l2_block {
                            let l1_block_attr =
                                get_l1_block_attributes(l1_provider.clone()).await?;

                            // Get the l2_to_l1_message_passer storage root
                            let proof =
                                l2_provider.latest()?.proof(l2_to_l1_message_passer, &[])?;

                            // TODO: Commit the proof at the block height to the db

                            // Propose the L2Output to the L2OutputOracle contract
                            //TODO: move this to a separate function
                            let _pending_txn = l2_oo
                                .proposeL2Output(
                                    proof.storage_root,
                                    U256::from(target_block),
                                    l1_block_attr.hash,
                                    U256::from(l1_block_attr.number),
                                )
                                .send()
                                .await?;

                            // TODO: Wait for transaction inclusion, and add retries
                            info!(
                                output_root = ?proof.storage_root,
                                l2_block_number = ?current_l2_block,
                                l1_block_hash = ?l1_block_attr.hash,
                                l1_block_number = ?l1_block_attr.number,
                                "Successfully Proposed L2Output"
                            );
                        } else {
                            //TODO: might not need this
                            continue;
                        }

                        //TODO: we need to check if the block is within the safe head, if not then
                        // continue
                        let safe_head = get_l2_safe_head(l1_provider.clone()).await?;

                        info!(committed_chain = ?new.range(), "Received commit");
                    }
                    ExExNotification::ChainReorged { old, new } => {
                        // TODO:
                        info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
                        // Fetch the updated latest block number
                        //TODO: delete entries from the db where the l2 block number is greater
                        // TODO: post the proof data if applicable or re run the logic
                    }
                    ExExNotification::ChainReverted { old } => {
                        // TODO:
                        info!(reverted_chain = ?old.range(), "Received revert");
                    }
                };
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
