use std::sync::Arc;

use alloy_network::{EthereumSigner, Network};
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_wallet::LocalWallet;
use alloy_sol_types::sol;
use alloy_transport::Transport;
use futures::Future;
use reth_exex::{ExExContext, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{BlockId, BlockNumberOrTag};
use reth_provider::StateProviderFactory;
use reth_tracing::tracing::info;
use rusqlite::Connection;

use crate::L2OutputOracle::L2OutputOracleCalls;
sol! {
    #[sol(rpc)]
    contract L2OutputOracle {
        function proposeL2Output(bytes32 _outputRoot, uint256 _l2BlockNumber, bytes32 _l1BlockHash, uint256 _l1BlockNumber) external payable;
        function latestBlockNumber() public view returns (uint256);
    }
    #[sol(rpc)]
    contract OptimismPortal {
        struct OutputRootProof {
            bytes32 version;
            bytes32 stateRoot;
            bytes32 messagePasserStorageRoot;
            bytes32 latestBlockhash;
        }

        struct WithdrawalTransaction {
            uint256 nonce;
            address sender;
            address target;
            uint256 value;
            uint256 gasLimit;
            bytes data;
        }
        function proveWithdrawalTransaction(WithdrawalTransaction memory _tx, uint256 _l2OutputIndex, OutputRootProof calldata _outputRootProof,bytes[] calldata _withdrawalProof) external;
    }
}

// ProvenWithdrawalParameters is the set of parameters to pass to the ProveWithdrawalTransaction
// and FinalizeWithdrawalTransaction functions
pub struct ProofWithdrawalParameters {
    nonce: u64,
    sender: Address,
    target: Address,
    value: u64,
    gas_limit: u64,
    l2_output_index: u64,
    data: Bytes,
    output_root_proof: OutputRootProof,
    withdrawal_proof: Vec<Bytes>,
}

pub struct OutputRootProof {
    version: Bytes,
    state_root: B256,
    message_passer_storage_root: B256,
    latest_blockhash: B256,
}

pub struct L2Output {
    output_root: B256,
    l2_block_number: u64,
    l1_block_hash: B256,
    l1_block_number: u64,
}

/// Create SQLite tables if they do not exist.
fn create_tables(connection: &mut Connection) -> rusqlite::Result<()> {
    // Create tables to store L2 outputs
    connection.execute(
        r#"
            CREATE TABLE IF NOT EXISTS deposits (
                output_root      TEXT PRIMARY KEY,
                l2_block_number     INTEGER NOT NULL,
                l1_block_hash          TEXT NOT NULL UNIQUE,
                l1_block_number INTEGER NOT NULL,
            );
            "#,
        (),
    )?;
    info!("Initialized database tables");

    Ok(())
}
#[derive(Debug, serde::Deserialize)]
struct ProposerConfig {
    l1_rpc: String,
    l2_output_oracle: Address,
    l2_to_l1_message_passer: Address,
    submission_interval: u64,
    proposer_private_key: String,
}

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    mut connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    create_tables(&mut connection)?;
    // TODO: Pull config from the config file
    // and init provider
    let config = serde_json::from_str::<ProposerConfig>(
        &std::fs::read_to_string("config.json").expect("Could not read config file"),
    )
    .unwrap();

    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .signer(EthereumSigner::from(config.proposer_private_key.parse::<LocalWallet>().unwrap()))
        .on_http(config.l1_rpc.parse().unwrap());
    Ok(OpProposer::new(
        provider,
        config.l2_output_oracle,
        config.l2_to_l1_message_passer,
        config.submission_interval,
    )
    .spawn(ctx, connection)?)
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
    submission_interval: u64,
    _network: std::marker::PhantomData<N>,
    _transport: std::marker::PhantomData<T>,
}

impl<T: Transport + Clone, N: Network, P: Provider<T, N>> OpProposer<T, N, P> {
    fn new(
        provider: P,
        l2_output_oracle: Address,
        l2_to_l1_message_passer: Address,
        submission_interval: u64,
    ) -> Self {
        Self {
            provider: Arc::new(provider),
            l2_output_oracle,
            l2_to_l1_message_passer,
            submission_interval,
            _network: std::marker::PhantomData,
            _transport: std::marker::PhantomData,
        }
    }

    fn spawn<Node: FullNodeComponents>(
        &self,
        mut ctx: ExExContext<Node>,
        _connection: Connection,
    ) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
        let l2_oo = L2OutputOracle::new(self.l2_output_oracle, self.provider.clone());
        let l2_provider = ctx.provider().clone();
        //TODO: initialization logic
        let fut = async move {
            while let Some(notification) = ctx.notifications.recv().await {
                match &notification {
                    ExExNotification::ChainCommitted { new } => {
                        let current_block = ctx.head.number.clone();

                        // TODO: Fetch the next block number from the L2OutputOracle contract
                        let next_block = l2_oo.latestBlockNumber().call().await?._0.to::<u64>();

                        if next_block == current_block {
                            let l1_block = self
                                .provider
                                .get_block(BlockId::Number(BlockNumberOrTag::Latest), false)
                                .await?;
                            if let Some(l1_block) = l1_block {
                                // Get the l2_to_l1_message_passer accounts root
                                let state_provider = l2_provider.state_by_block_id(
                                    BlockId::Number(BlockNumberOrTag::Number(current_block)),
                                )?;
                                let proof =
                                    state_provider.proof(self.l2_to_l1_message_passer, &[])?;
                                // Propose the L2Output to the L2OutputOracle contract
                                l2_oo
                                    .proposeL2Output(
                                        proof.storage_root,
                                        U256::from(next_block),
                                        l1_block.header.hash.unwrap(),
                                        U256::from(l1_block.header.number.unwrap()),
                                    )
                                    .send()
                                    .await?;

                                info!(
                                    output_root = ?proof.storage_root,
                                    l2_block_number = ?next_block,
                                    l1_block_hash = ?l1_block.header.hash.unwrap(),
                                    l1_block_number = ?l1_block.header.number.unwrap(),
                                    "Successfully Proposed L2Output"
                                );
                            }
                        }
                        info!(committed_chain = ?new.range(), "Received commit");
                    }
                    ExExNotification::ChainReorged { old, new } => {
                        // TODO:
                        info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
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
