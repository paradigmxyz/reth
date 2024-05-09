use alloy_primitives::{Address, Bytes, B256};
use alloy_sol_types::sol;
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
    Ok(OpProposer::new().spawn(ctx, connection)?)
}

async fn op_proposer_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    _connection: Connection,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
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
}

struct OpProposer {
    //TODO: persistent storage configuration
}

impl OpProposer {
    fn new() -> Self {
        Self {}
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
