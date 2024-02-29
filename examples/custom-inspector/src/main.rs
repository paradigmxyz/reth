//! Example of how to use a custom inspector to trace new pending transactions
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p custom-inspector --node --http --ws --recipients 0x....,0x....
//! ```
//!
//! If no recipients are specified, all transactions will be inspected.

#![warn(unused_crate_dependencies)]

use clap::Parser;
use futures_util::stream::StreamExt;
use reth::{
    cli::{
        components::{RethNodeComponents, RethRpcComponents, RethRpcServerHandles},
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
        Cli,
    },
    primitives::{Address, BlockId, IntoRecoveredTransaction},
    revm::{
        inspector_handle_register,
        interpreter::{Interpreter, OpCode},
        Database, Evm, EvmContext, Inspector,
    },
    rpc::{
        compat::transaction::transaction_to_call_request,
        eth::{revm_utils::EvmOverrides, EthTransactions},
    },
    tasks::TaskSpawner,
    transaction_pool::TransactionPool,
};
use std::collections::HashSet;

fn main() {
    Cli::<MyRethCliExt>::parse().run().unwrap();
}

/// The type that tells the reth CLI what extensions to use
struct MyRethCliExt;

impl RethCliExt for MyRethCliExt {
    /// This tells the reth CLI to trace addresses via `RethCliTxpoolExt`
    type Node = RethCliTxpoolExt;
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

impl RethNodeCommandConfig for RethCliTxpoolExt {
    /// Sets up a subscription to listen for new pending transactions and traces them.
    /// If the transaction is from one of the specified recipients, it will be traced.
    /// If no recipients are specified, all transactions will be traced.
    fn on_rpc_server_started<Conf, Reth>(
        &mut self,
        _config: &Conf,
        components: &Reth,
        rpc_components: RethRpcComponents<'_, Reth>,
        _handles: RethRpcServerHandles,
    ) -> eyre::Result<()>
    where
        Conf: RethRpcConfig,
        Reth: RethNodeComponents,
    {
        let recipients = self.recipients.iter().copied().collect::<HashSet<_>>();

        // create a new subscription to pending transactions
        let mut pending_transactions = components.pool().new_pending_pool_transactions_listener();

        let eth_api = rpc_components.registry.eth_api();

        println!("Spawning trace task!");
        // Spawn an async block to listen for transactions.
        components.task_executor().spawn(Box::pin(async move {
            // Waiting for new transactions
            while let Some(event) = pending_transactions.next().await {
                let tx = event.transaction;
                println!("Transaction received: {tx:?}");

                if recipients.is_empty() {
                    // convert the pool transaction
                    let call_request = transaction_to_call_request(tx.to_recovered_transaction());

                    let result = eth_api
                        .spawn_with_call_at(
                            call_request,
                            BlockId::default(),
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
        Ok(())
    }
}
