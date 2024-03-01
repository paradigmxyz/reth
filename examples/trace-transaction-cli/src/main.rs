//! Example of how to trace new pending transactions in the reth CLI
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p trace-transaction-cli -- node --http --ws --recipients 0x....,0x....
//! ```
//!
//! If no recipients are specified, all transactions will be traced.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use clap::Parser;
use futures_util::StreamExt;
use reth::{
    builder::NodeHandle,
    cli::Cli,
    primitives::{Address, IntoRecoveredTransaction},
    rpc::{
        compat::transaction::transaction_to_call_request,
        types::trace::{parity::TraceType, tracerequest::TraceCallRequest},
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
            let traceapi = node.rpc_registry.trace_api();

            println!("Spawning trace task!");
            // Spawn an async block to listen for transactions.
            node.task_executor.spawn(Box::pin(async move {
                // Waiting for new transactions
                while let Some(event) = pending_transactions.next().await {
                    let tx = event.transaction;
                    println!("Transaction received: {tx:?}");

                    if let Some(tx_recipient_address) = tx.to() {
                        if recipients.is_empty() || recipients.contains(&tx_recipient_address) {
                            // trace the transaction with `trace_call`
                            let callrequest =
                                transaction_to_call_request(tx.to_recovered_transaction());
                            let tracerequest = TraceCallRequest::new(callrequest)
                                .with_trace_type(TraceType::Trace);
                            if let Ok(trace_result) = traceapi.trace_call(tracerequest).await {
                                let hash = tx.hash();
                                println!("trace result for transaction {hash}: {trace_result:?}");
                            }
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
    /// recipients addresses that we want to trace
    #[arg(long, value_delimiter = ',')]
    pub recipients: Vec<Address>,
}
