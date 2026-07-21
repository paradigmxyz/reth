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

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::Address;
use alloy_rpc_types_eth::{state::EvmOverrides, TransactionRequest};
use clap::Parser;
use evm2::{
    interpreter::{opcode::OpCode, Interpreter},
    BaseEvmTypes, Inspector,
};
use futures_util::StreamExt;
use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, interface::Cli},
    node::{builder::NodeHandle, EthereumNode},
    pool::TransactionPool,
    rpc::api::eth::helpers::{Call, Trace},
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
                        let call_request =
                            TransactionRequest::from_recovered_transaction(tx.to_consensus());
                        let inspect_api = eth_api.clone();
                        let result = eth_api
                            .spawn_with_call_at(
                                call_request,
                                BlockNumberOrTag::Latest.into(),
                                EvmOverrides::default(),
                                move |db, evm_env, tx_env| {
                                    let mut inspector = DummyInspector::default();
                                    inspect_api.inspect(db, evm_env, &tx_env, &mut inspector)?;
                                    Ok(inspector)
                                },
                            )
                            .await;

                        if let Ok(inspector) = result {
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
        if let Some(opcode) = OpCode::new(interp.opcode()) {
            self.ret_val.push(format!("{}: {}", interp.pc(), opcode));
        }
    }
}
