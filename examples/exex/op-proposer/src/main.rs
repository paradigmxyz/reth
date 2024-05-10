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
use reth_primitives::{BlockId, BlockNumberOrTag, B256};
use reth_provider::{BlockNumReader, StateProviderFactory};
use reth_tracing::tracing::info;
use rusqlite::Connection;

sol! {
    #[sol(rpc)]
    contract L2OutputOracle {
        function proposeL2Output(bytes32 _outputRoot, uint256 _l2BlockNumber, bytes32 _l1BlockHash, uint256 _l1BlockNumber) external payable;
        function nextBlockNumber() public view returns (uint256);
    }
}

pub struct L2OutputDb {
    connection: Connection,
}

impl L2OutputDb {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn initialize(&mut self) -> eyre::Result<()> {
        // Create tables to store L2 outputs
        self.connection.execute(
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

    pub fn get_l2_output(&self, l2_block_number: u64) -> eyre::Result<Option<L2Output>> {
        todo!("not implemented yet")
    }

    pub fn insert_l2_output(&mut self, l2_output: L2Output) -> eyre::Result<()> {
        Ok(())
    }

    pub fn delete_l2_output(&mut self, l2_block_number: u64) -> eyre::Result<()> {
        Ok(())
    }
}

#[derive(Debug, serde::Deserialize)]
struct ProposerConfig {
    l1_rpc: String,
    l2_output_oracle: Address,
    l2_to_l1_message_passer: Address,
    proposer_private_key: String,
}

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    let config = serde_json::from_str::<ProposerConfig>(
        &std::fs::read_to_string("config.json").expect("Could not read config file"),
    )
    .unwrap();

    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .signer(EthereumSigner::from(config.proposer_private_key.parse::<LocalWallet>().unwrap()))
        .on_http(config.l1_rpc.parse().unwrap());

    let l2_output_db = L2OutputDb::new(connection);
    Ok(OpProposer::new(provider, config.l2_output_oracle, config.l2_to_l1_message_passer)
        .spawn(ctx, l2_output_db)?)
}

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

#[derive(Clone)]
pub struct L2Output {
    pub output_root: B256,
    pub l2_block_number: u64,
    pub l1_block_hash: B256,
    pub l1_block_number: u64,
}

struct OpProposer<T, N, P>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    provider: Arc<P>,
    l2_output_oracle: Address,
    l2_to_l1_message_passer: Address,
    _pd: PhantomData<fn() -> (T, N)>,
}

impl<T: Transport + Clone, N: Network, P: Provider<T, N>> OpProposer<T, N, P> {
    fn new(provider: P, l2_output_oracle: Address, l2_to_l1_message_passer: Address) -> Self {
        Self {
            provider: Arc::new(provider),
            l2_output_oracle,
            l2_to_l1_message_passer,
            _pd: PhantomData,
        }
    }

    fn spawn<Node: FullNodeComponents>(
        &self,
        mut ctx: ExExContext<Node>,
        mut l2_output_db: L2OutputDb,
    ) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
        let l2_output_oracle = L2OutputOracle::new(self.l2_output_oracle, self.provider.clone());
        let l2_provider = ctx.provider().clone();
        let l1_provider = self.provider.clone();
        let l2_to_l1_message_passer = self.l2_to_l1_message_passer.clone();

        let fut = async move {
            while let Some(notification) = ctx.notifications.recv().await {
                info!(?notification, "Received ExEx notification");

                //TODO: package this into function
                match &notification {
                    ExExNotification::ChainReorged { old, new } => {
                        // for block in old..new {
                        //     l2_output_db.delete_l2_output(block)?;
                        // }
                    }
                    ExExNotification::ChainReverted { old } => {
                        // TODO:
                    }
                    _ => {}
                };

                let target_block = l2_output_oracle.nextBlockNumber().call().await?._0.to::<u64>();
                let current_l2_block = l2_provider.last_block_number()?;

                //TODO: we need to check if the block is within the safe head

                // Get the l2 output data to prepare for submission
                let l2_output = if target_block < current_l2_block {
                    //TODO: check the "transaction manager" to see if the proof has already
                    l2_output_db.get_l2_output(target_block)?.ok_or(eyre!("L2 output not found"))?
                } else if target_block == current_l2_block {
                    let l1_block_attr = get_l1_block_attributes(&l1_provider).await?;
                    let proof = l2_provider.latest()?.proof(l2_to_l1_message_passer, &[])?;

                    let l2_output = L2Output {
                        output_root: proof.storage_root,
                        l2_block_number: target_block,
                        l1_block_hash: l1_block_attr.hash,
                        l1_block_number: l1_block_attr.number,
                    };

                    // TODO: Commit the proof at the block height to the db

                    l2_output_db.insert_l2_output(l2_output.clone())?;

                    l2_output
                } else {
                    continue;
                };

                // Submit a transaction to propose the L2Output to the L2OutputOracle contract
                //TODO: transaction management
                let _pending_txn = l2_output_oracle
                    .proposeL2Output(
                        l2_output.output_root,
                        U256::from(target_block),
                        l2_output.l1_block_hash,
                        U256::from(l2_output.l1_block_number),
                    )
                    .send()
                    .await?;

                // // TODO: Wait for transaction inclusion, and add retries
                // info!(
                //     output_root = ?proof.storage_root,
                //     l2_block_number = ?current_l2_block,
                //     l1_block_hash = ?l1_block_attr.hash,
                //     l1_block_number = ?l1_block_attr.number,
                //     "Successfully Proposed L2Output"
                // );
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
