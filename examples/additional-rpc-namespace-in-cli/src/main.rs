//! Example of how to use additional rpc namespaces in the reth CLI
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p additional-rpc-namespace-in-cli -- node --http --ws --enable-ext
//! ```
//!
//! This installs an additional RPC method `txpoolExt_transactionCount` that can be queried via [cast](https://github.com/foundry-rs/foundry)
//!
//! ```sh
//! cast rpc txpoolExt_transactionCount
//! ```

use clap::Parser;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth::cli::Cli;
use reth_node_ethereum::EthereumNode;
use reth_transaction_pool::TransactionPool;

fn main() {
    Cli::<RethCliTxpoolExt>::parse()
        .run(|builder, args| async move {
            let handle = builder
                .node(EthereumNode::default())
                .extend_rpc_modules(move |ctx| {
                    if !args.enable_ext {
                        return Ok(())
                    }

                    // here we get the configured pool.
                    let pool = ctx.pool().clone();

                    let ext = TxpoolExt { pool };

                    // now we merge our extension namespace into all configured transports
                    ctx.modules.merge_configured(ext.into_rpc())?;

                    println!("txpool extension enabled");

                    Ok(())
                })
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
struct RethCliTxpoolExt {
    /// CLI flag to enable the txpool extension namespace
    #[arg(long)]
    pub enable_ext: bool,
}

/// trait interface for a custom rpc namespace: `txpool`
///
/// This defines an additional namespace where all methods are configured as trait functions.
#[cfg_attr(not(test), rpc(server, namespace = "txpoolExt"))]
#[cfg_attr(test, rpc(server, client, namespace = "txpoolExt"))]
pub trait TxpoolExtApi {
    /// Returns the number of transactions in the pool.
    #[method(name = "transactionCount")]
    fn transaction_count(&self) -> RpcResult<usize>;
}

/// The type that implements the `txpool` rpc namespace trait
pub struct TxpoolExt<Pool> {
    pool: Pool,
}

impl<Pool> TxpoolExtApiServer for TxpoolExt<Pool>
where
    Pool: TransactionPool + Clone + 'static,
{
    fn transaction_count(&self) -> RpcResult<usize> {
        Ok(self.pool.pool_size().total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::{http_client::HttpClientBuilder, server::ServerBuilder};
    use reth_transaction_pool::noop::NoopTransactionPool;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_call_transaction_count_http() {
        let server_addr = start_server().await;
        let uri = format!("http://{}", server_addr);
        let client = HttpClientBuilder::default().build(&uri).unwrap();
        let count = TxpoolExtApiClient::transaction_count(&client).await.unwrap();
        assert_eq!(count, 0);
    }

    async fn start_server() -> std::net::SocketAddr {
        let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let pool = NoopTransactionPool::default();
        let api = TxpoolExt { pool };
        let server_handle = server.start(api.into_rpc());

        tokio::spawn(server_handle.stopped());

        addr
    }
}
