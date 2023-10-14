//! Example of how to trace new pending transactions in the reth CLI
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p trace-transaction-cli -- node --http --ws --receipient RECEIPIENT_ADDRESS
//! ```
use clap::Parser;
use jsonrpsee::tokio;
use reth::{
    cli::{
        components::{RethNodeComponents, RethRpcComponents, RethRpcServerHandles},
        config::RethRpcConfig,
        ext::{RethCliExt, RethNodeCommandConfig},
        Cli,
    },
    providers::TransactionsProvider,
    transaction_pool::TransactionPool,
};
use reth_primitives::Address;

fn main() {
    Cli::<MyRethCliExt>::parse().run().unwrap();
}

/// The type that tells the reth CLI what extensions to use
struct MyRethCliExt;

impl RethCliExt for MyRethCliExt {
    /// This tells the reth CLI to install the `txpool` rpc namespace via `RethCliTxpoolExt`
    type Node = RethCliTxpoolExt;
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
struct RethCliTxpoolExt {
    /// CLI flag to enable the txpool extension namespace
    #[clap(long)]
    pub receipient: Address,
}

impl RethNodeCommandConfig for RethCliTxpoolExt {
    fn on_rpc_server_started<Conf, Reth>(
        &mut self,
        _config: &Conf,
        _components: &Reth,
        rpc_components: RethRpcComponents<'_, Reth>,
        _handles: RethRpcServerHandles,
    ) -> eyre::Result<()>
    where
        Conf: RethRpcConfig,
        Reth: RethNodeComponents,
    {
        println!("RPC Server has started!");
        let provider = rpc_components.registry.provider().clone();
        let receipient = self.receipient;
        if let Some(eth_handlers) = rpc_components.registry.eth.clone() {
            let api = eth_handlers.api;
            let tx_pool = api.pool();
            let mut tx_subscription = tx_pool.pending_transactions_listener();

            // Spawn an async block to listen for transactions.
            tokio::spawn(async move {
                // Awaiting for a new transaction and print it.
                while let Some(tx) = tx_subscription.recv().await {
                    println!("Transaction received: {:?}", tx);
                    let res = provider.transaction_by_hash(tx);
                    if let Ok(Some(tx_receipient)) = res {
                        if tx_receipient.kind().to() == Some(receipient) {
                            // TODO: CAll trace_call
                        }
                    }
                }
            });
        }

        Ok(())
    }
}
