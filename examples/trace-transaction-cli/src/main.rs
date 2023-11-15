//! Example of how to trace new pending transactions in the reth CLI
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p trace-transaction-cli -- node --http --ws --recipients 0x....,0x....
//! ```
//!
//! If no recipients are specified, all transactions will be traced.
use clap::Parser;
use futures_util::StreamExt;
use reth::{
    cli::{
        components::{RethNodeComponents, RethRpcComponents, RethRpcServerHandles},
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
        Cli,
    },
    primitives::{Address, IntoRecoveredTransaction},
    rpc::{
        compat::transaction::transaction_to_call_request,
        types::trace::{parity::TraceType, tracerequest::TraceCallRequest},
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
    /// recipients addresses that we want to trace
    #[arg(long, value_delimiter = ',')]
    pub recipients: Vec<Address>,
}

impl RethNodeCommandConfig for RethCliTxpoolExt {
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

        // get an instance of the `trace_` API handler
        let traceapi = rpc_components.registry.trace_api();

        println!("Spawning trace task!");
        // Spawn an async block to listen for transactions.
        components.task_executor().spawn(Box::pin(async move {
            // Waiting for new transactions
            while let Some(event) = pending_transactions.next().await {
                let tx = event.transaction;
                println!("Transaction received: {:?}", tx);

                if let Some(tx_recipient_address) = tx.to() {
                    if recipients.is_empty() || recipients.contains(&tx_recipient_address) {
                        // trace the transaction with `trace_call`
                        let callrequest =
                            transaction_to_call_request(tx.to_recovered_transaction());
                        let tracerequest =
                            TraceCallRequest::new(callrequest).with_trace_type(TraceType::Trace);
                        if let Ok(trace_result) = traceapi.trace_call(tracerequest).await {
                            println!(
                                "trace result for transaction : {:?} is {:?}",
                                tx.hash(),
                                trace_result
                            );
                        }
                    }
                }
            }
        }));
        Ok(())
    }
}
