//! Example of how to trace new pending transactions in the reth CLI
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p trace-transaction-cli -- node --http --ws --receipients Vec<Address>
//! ```
use clap::Parser;
use reth::{
    cli::{
        components::{RethNodeComponents, RethRpcComponents, RethRpcServerHandles},
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
        Cli,
    },
    primitives::{Address, BlockId},
    providers::TransactionsProvider,
    rpc::{
        compat::transaction::to_call_request,
        types::{state::StateOverride, trace::parity::TraceType, BlockOverrides},
    },
    tasks::TaskSpawner,
    transaction_pool::TransactionPool,
};
use std::{collections::HashSet, sync::Arc};
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
    /// receipients addresses that we want to trace
    #[arg(long, value_delimiter = ',')]
    pub receipients: Vec<Address>,
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
        println!("RPC Server has started!");
        let provider = components.provider().clone();
        let receipients = Arc::new(self.receipients.clone()); // Clone into an Arc
        let traceapi = rpc_components.registry.trace_api();
        let eth_handlers = rpc_components.registry.eth_handlers().clone();
        let api = eth_handlers.api;
        let tx_pool = api.pool();
        let mut tx_subscription = tx_pool.pending_transactions_listener();

        // Spawn an async block to listen for transactions.
        components.task_executor().spawn(Box::pin(async move {
            // Awaiting for a new transaction and print it.
            while let Some(tx) = tx_subscription.recv().await {
                println!("Transaction received: {:?}", tx);
                let res = provider.transaction_by_hash(tx);
                if let Ok(Some(tx_receipient)) = res {
                    if let Some(tx_recipient_address) = tx_receipient.kind().to() {
                        if receipients.contains(&tx_recipient_address) {
                            let blockdetails = provider.transaction_by_hash_with_meta(tx);

                            let base_fee = match blockdetails {
                                Ok(Some((_, meta))) => meta.base_fee,
                                Ok(None) => Some(u64::default()),
                                Err(_e) => Some(u64::default()),
                            };
                            let callrequest = to_call_request(tx_receipient, base_fee);
                            let mut tracetype = HashSet::new();
                            tracetype.insert(TraceType::Trace);
                            let block_id: Option<BlockId> = None;
                            let state_override: Option<StateOverride> = None;
                            let block_override: Option<BlockOverrides> = None;
                            let boxed_block_override = block_override.map(Box::new);
                            let trace_result = traceapi
                                .trace_call(
                                    callrequest,
                                    tracetype,
                                    block_id,
                                    state_override,
                                    boxed_block_override,
                                )
                                .await;
                            println!(
                                "trace result for transaction : {:?} is {:?}",
                                tx, trace_result
                            );
                        }
                    }
                }
            }
        }));
        Ok(())
    }
}
