//! Example of how to use a custom inspector to trace new pending transactions
//!
//! Run with
//!
//! ```sh
//! cargo run --release -p example-custom-inspector -- node --http --ws --recipients 0x....,0x....
//! ```
//!
//! If no recipients are specified, all transactions will be inspected.

#![warn(unused_crate_dependencies)]

use alloy_primitives::Address;
use clap::Parser;
use evm2::{
    evm::{CacheDB, Db},
    interpreter::{opcode::OpCode, Interpreter},
    BaseEvmTypes, Inspector,
};
use futures_util::StreamExt;
use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, interface::Cli},
    evm::{
        primitives::{database::StateProviderDatabase, ConfigureEvm},
        EthTxEnv,
    },
    node::{builder::NodeHandle, EthereumNode},
    pool::TransactionPool,
    rpc::api::eth::helpers::Trace,
    storage::{BlockReaderIdExt, StateProviderFactory},
};

fn main() {
    Cli::<EthereumChainSpecParser, RethCliTxpoolExt>::parse()
        .run(async move |builder, args| {
            // launch the node
            let NodeHandle { node, node_exit_future } =
                builder.node(EthereumNode::default()).launch().await?;

            // create a new subscription to pending transactions
            let mut pending_transactions = node.pool.new_pending_pool_transactions_listener();

            // get an instance of the `eth_` API handler
            let eth_api = node.rpc_registry.eth_api().clone();

            println!("Spawning inspect task!");

            // Spawn an async block to listen for transactions.
            node.task_executor.spawn_task(async move {
                // Waiting for new transactions
                while let Some(event) = pending_transactions.next().await {
                    let tx = event.transaction;
                    println!("Transaction received: {tx:?}");

                    if let Some(recipient) = tx.to() &&
                        args.is_match(&recipient)
                    {
                        let Some(header) = (match node.provider.latest_header() {
                            Ok(header) => header,
                            Err(err) => {
                                eprintln!("failed to load latest header: {err}");
                                continue;
                            }
                        }) else {
                            eprintln!("latest header is unavailable");
                            continue;
                        };

                        let state = match node.provider.latest() {
                            Ok(state) => state,
                            Err(err) => {
                                eprintln!("failed to load latest state: {err}");
                                continue;
                            }
                        };

                        let evm_env = match node.evm_config.evm_env(header.header()) {
                            Ok(env) => env,
                            Err(err) => match err {},
                        };
                        let tx_env = EthTxEnv::from(tx.to_consensus());

                        let mut db = CacheDB::new(Db::new(StateProviderDatabase::new(state)));
                        let result =
                            eth_api.inspect(&mut db, evm_env, &tx_env, DummyInspector::default());

                        if let Ok((inspector, _)) = result {
                            let hash = tx.hash();
                            println!(
                                "Inspector result for transaction {}:\n{}",
                                hash,
                                inspector.ret_val.join("\n")
                            );
                        }
                    }
                }
            });

            node_exit_future.await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Default, clap::Args)]
struct RethCliTxpoolExt {
    /// The addresses of the recipients that we want to inspect.
    #[arg(long, value_delimiter = ',')]
    pub recipients: Vec<Address>,
}

impl RethCliTxpoolExt {
    /// Check if the recipient is in the list of recipients to inspect.
    pub fn is_match(&self, recipient: &Address) -> bool {
        self.recipients.is_empty() || self.recipients.contains(recipient)
    }
}

/// A dummy inspector that logs the opcodes and their corresponding program counter.
#[derive(Default, Debug, Clone)]
struct DummyInspector {
    ret_val: Vec<String>,
}

impl Inspector<BaseEvmTypes> for DummyInspector {
    fn step(&mut self, interp: &mut Interpreter<'_, '_, BaseEvmTypes>) {
        let opcode = OpCode::new_or_unknown(interp.opcode());
        self.ret_val.push(format!("{}: {}", interp.pc(), opcode));
    }
}
