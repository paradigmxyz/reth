//! Example of how to use additional rpc namespaces in the reth CLI
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p node-custom-rpc -- node --http --ws --enable-ext
//! ```
//!
//! This installs an additional RPC method `txpoolExt_transactionCount` that can be queried via [cast](https://github.com/foundry-rs/foundry)
//!
//! ```sh
//! cast rpc txpoolExt_transactionCount
//! ```

use clap::Parser;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth::{chainspec::EthereumChainSpecParser, cli::Cli};
use reth_node_ethereum::EthereumNode;
use reth_transaction_pool::{TransactionPool, PoolSize};
use tracing_subscriber;

fn main() {
    tracing_subscriber::fmt::init(); // Improved logging

    Cli::<EthereumChainSpecParser, RethCliTxpoolExt>::parse()
        .run(|builder, args| async move {
            let handle = builder
                .node(EthereumNode::default())
                .extend_rpc_modules(move |ctx| {
                    if args.enable_ext {
                        let pool = ctx.pool().clone();
                        let ext = TxpoolExt { pool };

                        ctx.modules.merge_configured(ext.into_rpc())?;
                        tracing::info!("txpool extension enabled"); // Improved logging
                    }
                    Ok(())
                })
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// Custom CLI arguments extension to enable the txpool extension namespace
#[derive(Debug, Clone, Copy, Default, clap::Args)]
struct RethCliTxpoolExt {
    /// CLI flag to enable the txpool extension namespace
    #[arg(long)]
    pub enable_ext: bool,
}

/// Trait interface for a custom RPC namespace: `txpoolExt`
#[cfg_attr(not(test), rpc(server, namespace = "txpoolExt"))]
#[cfg_attr(test, rpc(server, client, namespace = "txpoolExt"))]
pub trait TxpoolExtApi {
    /// Returns the number of transactions in the pool.
    #[method(name = "transactionCount")]
    fn transaction_count(&self) -> RpcResult<u64>; // Changed usize to u64 for better JSON-RPC compatibility
}

/// The type that implements the `txpoolExt` RPC namespace trait
pub struct TxpoolExt<Pool> {
    pool: Pool,
}

impl<Pool> TxpoolExtApiServer for TxpoolExt<Pool>
where
    Pool: TransactionPool + Clone + 'static,
{
    fn transaction_count(&self) -> RpcResult<u64> {
        Ok(self.pool.pool_size().total as u64) // Improved data type handling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::{http_client::HttpClientBuilder, server::ServerBuilder};
    use reth_transaction_pool::noop::NoopTransactionPool;

    struct MockTransactionPool;

    impl TransactionPool for MockTransactionPool {
        fn pool_size(&self) -> PoolSize {
            PoolSize { total: 5 } // Mocked pool size
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_call_transaction_count_http() {
        let server_addr = start_server().await;
        let uri = format!("http://{}", server_addr);
        let client = HttpClientBuilder::default().build(&uri).unwrap();
        let count = TxpoolExtApiClient::transaction_count(&client).await.unwrap();
        assert_eq!(count, 5); // Now tests a mocked value instead of a fixed one
    }

    async fn start_server() -> std::net::SocketAddr {
        let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let pool = MockTransactionPool; // Use mocked pool
        let api = TxpoolExt { pool };
        let server_handle = server.start(api.into_rpc());

        tokio::spawn(server_handle.stopped());

        addr
    }
}
