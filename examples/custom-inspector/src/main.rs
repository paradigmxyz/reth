//! Example of how to use a custom inspector to trace new pending transactions
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p custom-inspector -- node --http --ws --recipients 0x....,0x....
//! ```
//!
//! If no recipients are specified, all transactions will be inspected.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use clap::Parser;
use futures_util::StreamExt;
use reth::{
    builder::NodeHandle,
    cli::Cli,
    primitives::{Address, BlockNumberOrTag, IntoRecoveredTransaction},
    revm::{
        inspector_handle_register,
        interpreter::{Interpreter, OpCode},
        Database, Evm, EvmContext, Inspector,
    },
    rpc::{
        compat::transaction::transaction_to_call_request,
        eth::{revm_utils::EvmOverrides, EthTransactions},
    },
    transaction_pool::TransactionPool,
};
use reth_node_ethereum::node::EthereumNode;
use std::collections::HashSet;

fn main() {
    Cli::<RethCliTxpoolExt>::parse()
        .run(|builder, args| async move {
            // launch the node
            let NodeHandle { mut node, node_exit_future } =
                builder.node(EthereumNode::default()).launch().await?;

            let recipients = args.recipients.iter().copied().collect::<HashSet<_>>();

            // create a new subscription to pending transactions
            let mut pending_transactions = node.pool.new_pending_pool_transactions_listener();

            // get an instance of the `trace_` API handler
            let eth_api = node.rpc_registry.eth_api();

            println!("Spawning trace task!");

            // Spawn an async block to listen for transactions.
            node.task_executor.spawn(Box::pin(async move {
                // Waiting for new transactions
                while let Some(event) = pending_transactions.next().await {
                    let tx = event.transaction;
                    println!("Transaction received: {tx:?}");

                    if recipients.is_empty() {
                        // convert the pool transaction
                        let call_request =
                            transaction_to_call_request(tx.to_recovered_transaction());

                        let result = eth_api
                            .spawn_with_call_at(
                                call_request,
                                BlockNumberOrTag::Latest.into(),
                                EvmOverrides::default(),
                                move |db, env| {
                                    let mut dummy_inspector = DummyInspector::default();
                                    {
                                        // configure the evm with the custom inspector
                                        let mut evm = Evm::builder()
                                            .with_db(db)
                                            .with_external_context(&mut dummy_inspector)
                                            .with_env_with_handler_cfg(env)
                                            .append_handler_register(inspector_handle_register)
                                            .build();
                                        // execute the transaction on a blocking task and await the
                                        // inspector result
                                        let _ = evm.transact()?;
                                    }
                                    Ok(dummy_inspector)
                                },
                            )
                            .await;

                        if let Ok(ret_val) = result {
                            let hash = tx.hash();
                            println!(
                                "Inspector result for transaction {}: \n {}",
                                hash,
                                ret_val.ret_val.join("\n")
                            );
                        }
                    }
                }
            }));

            node_exit_future.await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Default, clap::Args)]
struct RethCliTxpoolExt {
    /// The addresses of the recipients that we want to trace.
    #[arg(long, value_delimiter = ',')]
    pub recipients: Vec<Address>,
}

/// A dummy inspector that logs the opcodes and their corresponding program counter for a
/// transaction
#[derive(Default, Debug, Clone)]
struct DummyInspector {
    ret_val: Vec<String>,
}

impl<DB> Inspector<DB> for DummyInspector
where
    DB: Database,
{
    /// This method is called at each step of the EVM execution.
    /// It checks if the current opcode is valid and if so, it stores the opcode and its
    /// corresponding program counter in the `ret_val` vector.
    fn step(&mut self, interp: &mut Interpreter, _context: &mut EvmContext<DB>) {
        if let Some(opcode) = OpCode::new(interp.current_opcode()) {
            self.ret_val.push(format!("{}: {}", interp.program_counter(), opcode));
        }
    }
}
