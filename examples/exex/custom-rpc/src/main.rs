use alloy_rpc_types::{BlockNumberOrTag, Filter};
use alloy_sol_types::{sol, SolEventInterface};
use clap::{Id, Parser};
use jsonrpsee::{core::RpcResult, proc_macros::rpc, types::error::ErrorObject};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{BlockNumber, Log, SealedBlockWithSenders, TransactionSigned};
use reth_provider::{BlockReaderIdExt, Chain};
sol!(L1StandardBridge, "l1_standard_bridge_abi.json");
use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated, L1StandardBridgeEvents};
use std::sync::{Arc, Mutex, MutexGuard};
use std::default::Default;
use std::{path::Path};
use reth::{
    primitives::ChainSpecBuilder,
    providers::{providers::BlockchainProvider, ProviderFactory},
    utils::db::open_db_read_only,
};
use reth_node_ethereum::EthEvmConfig;

use reth::{
    blockchain_tree::noop::NoopBlockchainTree, providers::test_utils::TestCanonStateSubscriptions,
    tasks::TokioTaskExecutor,
};
use reth_db::{mdbx::DatabaseArguments, models::client_version::ClientVersion};
// Bringing up the RPC
use reth::rpc::builder::{
    RethRpcModule, RpcModuleBuilder, RpcServerConfig, TransportRpcModuleConfig,
};
#[derive(Debug)]
struct Deposit {
    id: i32,
    block_number: u64,
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
struct RethCliOpDepositCountExt {
    #[arg(long)]
    pub enable_ext: bool,
}

/// trait interface for a custom rpc namespace: `opdepositcount`
///
/// This defines an additional namespace where all methods are configured as trait functions.
#[rpc[server, namespace="onDepositCounts"]]
pub trait OpDepositCountExtApi {
    #[method(name = "opdepositCount")]
    fn op_deposit_count(&self) -> RpcResult<usize>;
}

pub struct OpDepositCountExt<Provider> {
    provider: Provider,
}

impl<P> OpDepositCountExt<P>
where
    P: BlockReaderIdExt +'static,
{
    pub fn new(provider: P) -> OpDepositCountExt<P> {
        Self { provider }
    }
}

impl<P> OpDepositCountExtApiServer for OpDepositCountExt<P>
where
    P: BlockReaderIdExt + 'static,
{
 fn op_deposit_count(&self) -> RpcResult<usize> {
        
        todo!()
        
}
}
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_tracing::tracing::info;
use rusqlite::{params, Connection};

struct OpCount<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    db: Database,
}

impl<Node: FullNodeComponents> OpCount<Node> {
    fn new(ctx: ExExContext<Node>, connection: Connection) -> eyre::Result<Self> {
        let db = Database::new(connection)?;
        Ok(Self { ctx, db })
    }

    async fn start(mut self) -> eyre::Result<()> {
        // Process all new chain state notifications
        while let Some(notification) = self.ctx.notifications.recv().await {
            // Revert all deposits and withdrawals
            if let Some(reverted_chain) = notification.reverted_chain() {
                let events = decode_chain_into_events(&reverted_chain);

                let mut deposits = 0;

                for (_, tx, _, event) in events {
                    match event {
                        // L1 -> L2 deposit
                        L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                            ..
                        }) => {
                            let deleted = self.db.connection().execute(
                                "DELETE FROM deposits WHERE tx_hash = ?;",
                                (tx.hash().to_string(),),
                            )?;
                            deposits += deleted;
                        }
                        // L2 -> L1 withdrawal
                        L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                            ..
                        }) => {}
                        _ => continue,
                    }
                }

                info!(block_range = ?reverted_chain.range(), %deposits, "Reverted chain events");
            }

            // Insert all new deposits and withdrawals
            if let Some(committed_chain) = notification.committed_chain() {
                let events = decode_chain_into_events(&committed_chain);

                let mut deposits = 0;

                for (block, tx, log, event) in events {
                    match event {
                        // L1 -> L2 deposit
                        L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                            amount,
                            from,
                            to,
                            ..
                        }) => {
                            let inserted = self.db.connection().execute(
                                r#"
                                INSERT INTO deposits (block_number, tx_hash)
                                VALUES (?, ?)
                                "#,
                                (
                                    block.number,
                                    tx.hash().to_string(),
                                ),
                            )?;
                            deposits += inserted;
                        }
                        // L2 -> L1 withdrawal
                        L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                            amount,
                            from,
                            to,
                            ..
                        }) => {}
                        _ => continue,
                    };
                }

                info!(block_range = ?committed_chain.range(), %deposits, "Committed chain events");

                // Send a finished height event, signaling the node that we don't need any blocks
                // below this height anymore
                self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Ok(())
    }
}

pub struct Database {
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    /// Create new database with the provided connection.
    pub fn new(connection: Connection) -> eyre::Result<Self> {
        let database = Self { connection: Arc::new(Mutex::new(connection)) };
        database.create_tables()?;
        Ok(database)
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().expect("failed to acquire database lock")
    }

    pub fn get_op_deposit_count(
        &self,
        block_number: BlockNumber,
    ) -> eyre::Result<Option<u64>> {
        let mut deposit_count = 0;
        let connection = self.connection();
        let mut stmt = connection
            .prepare("SELECT id, block_number FROM deposits WHERE block_number = ?")?;

        let deposit_iter = stmt.query_map([block_number], |row| {
            Ok(Deposit { id: row.get(0)?, block_number: row.get(1)? })
        })?;

        for deposit in deposit_iter {
            match deposit {
                Ok(_dep) => {
                    deposit_count += 1;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
        Ok(Some(deposit_count))
    }

    /// Create SQLite tables if they do not exist.
    fn create_tables(&self) -> rusqlite::Result<()> {
        // Create deposits and withdrawals tables
        self.connection().execute(
            r#"
            CREATE TABLE IF NOT EXISTS deposits (
                id               INTEGER PRIMARY KEY,
                block_number     INTEGER NOT NULL
                tx_hash          TEXT NOT NULL UNIQUE,
            );
            "#,
            (),
        )?;

        Ok(())
    }
}

/// Decode chain of blocks into a flattened list of receipt logs, and filter only
/// [L1StandardBridgeEvents].
fn decode_chain_into_events(
    chain: &Chain,
) -> impl Iterator<Item = (&SealedBlockWithSenders, &TransactionSigned, &Log, L1StandardBridgeEvents)>
{
    chain
        // Get all blocks and receipts
        .blocks_and_receipts()
        // Get all receipts
        .flat_map(|(block, receipts)| {
            block
                .body
                .iter()
                .zip(receipts.iter().flatten())
                .map(move |(tx, receipt)| (block, tx, receipt))
        })
        // Get all logs
        .flat_map(|(block, tx, receipt)| receipt.logs.iter().map(move |log| (block, tx, log)))
        // Decode and filter bridge events
        .filter_map(|(block, tx, log)| {
            L1StandardBridgeEvents::decode_raw_log(log.topics(), &log.data.data, true)
                .ok()
                .map(|event| (block, tx, log, event))
        })
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("OPDepositCount", |ctx| async move {
                let connection = Connection::open("op_deposit_count.db")?;
                Ok(OpCount::new(ctx, connection)?.start())
            })
            .launch()
            .await?;

    // 1. Setup the DB
    let db_path = std::env::var("op_deposit_count.db")?;
    let db_path = Path::new(&db_path);
    let db = Arc::new(open_db_read_only(
        db_path.join("db").as_path(),
        DatabaseArguments::new(ClientVersion::default()),
    )?);
    let spec = Arc::new(ChainSpecBuilder::mainnet().build());
    let factory = ProviderFactory::new(db.clone(), spec.clone(), db_path.join("static_files"))?;

    // 2. Setup the blockchain provider using only the database provider and a noop for the tree to
    //    satisfy trait bounds. Tree is not used in this example since we are only operating on the
    //    disk and don't handle new blocks/live sync etc, which is done by the blockchain tree.
    let provider = BlockchainProvider::new(factory, Arc::new(NoopBlockchainTree::default()))?;

    let rpc_builder = RpcModuleBuilder::default()
        .with_provider(provider.clone())
        // Rest is just noops that do nothing
        .with_noop_pool()
        .with_noop_network()
        .with_executor(TokioTaskExecutor::default())
        .with_evm_config(EthEvmConfig::default())
        .with_events(TestCanonStateSubscriptions::default());

    // Pick which namespaces to expose.
    let config = TransportRpcModuleConfig::default().with_http([RethRpcModule::Eth]);
    let mut server = rpc_builder.build(config);

    // Add a custom rpc namespace
    let custom_rpc = OpDepositCountExt { provider };
    server.merge_configured(custom_rpc.into_rpc())?;

    // Start the server & keep it alive
    let server_args =
        RpcServerConfig::http(Default::default()).with_http_address("0.0.0.0:8545".parse()?);
    let _handle = server_args.start(server).await?;
    futures::future::pending::<()>().await;

        handle.wait_for_node_exit().await
})
}
