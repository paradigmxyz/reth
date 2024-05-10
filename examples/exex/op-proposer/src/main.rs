use std::sync::Arc;

use alloy_network::Network;
use alloy_primitives::{Address, Bytes, B256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer::k256::ecdsa::SigningKey;
use alloy_signer_wallet::LocalWallet;
use alloy_sol_types::sol;
use alloy_transport::Transport;
use futures::Future;
use reth_exex::{ExExContext, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use rusqlite::Connection;
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

async fn init_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    mut connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    create_tables(&mut connection)?;
    // TODO: Pull config from the config file
    // and init provider
    Ok(OpProposer::new().spawn(ctx, connection)?)
}

struct OpProposer<T, N, P>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    signer: LocalWallet,
    provider: Arc<P>,
    l2_output_oracle: Address,
    l2_to_l1_message_passer: Address,
    submission_interval: u64,
    _network: std::marker::PhantomData<N>,
    _transport: std::marker::PhantomData<T>,
}

impl<T: Transport + Clone, N: Network, P: Provider<T, N>> OpProposer<T, N, P> {
    fn new(
        pk: &str,
        provider: P,
        l2_output_oracle: Address,
        l2_to_l1_message_passer: Address,
        submission_interval: u64,
    ) -> Self {
        let signer = pk.parse().unwrap();
        Self {
            signer,
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
        //TODO: initialization logic

        let fut = async move {
            while let Some(notification) = ctx.notifications.recv().await {
                match &notification {
                    ExExNotification::ChainCommitted { new } => {
                        // TODO: Fetch the next block number from the L2OutputOracle contract
                        // If the next block number is equal to the current cononical tip
                        //  - Get the account root of the l2_to_l1_message_passer contract
                        //  - Construct the L2Output.
                        //  - Send the L2Output to the L2OutputOracle contract
                        //  - Write the L2Output to the database
                        info!(committed_chain = ?new.range(), "Received commit");
                    }
                    ExExNotification::ChainReorged { old, new } => {
                        info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
                    }
                    ExExNotification::ChainReverted { old } => {
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
