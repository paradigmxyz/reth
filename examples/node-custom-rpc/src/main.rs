//! Example of how to use additional rpc namespaces in the reth CLI
//!
//! Run with
//!
//! ```sh
//! cargo run -p node-custom-rpc -- node --http --ws --enable-ext
//! ```
//!
//! This installs an additional RPC method `txpoolExt_transactionCount` that can be queried via [cast](https://github.com/foundry-rs/foundry)
//!
//! ```sh
//! cast rpc txpoolExt_transactionCount
//! ```

#![warn(unused_crate_dependencies)]

use clap::Parser;
use jsonrpsee::{
    core::{RpcResult, SubscriptionResult},
    proc_macros::rpc,
    PendingSubscriptionSink, SubscriptionMessage,
};
use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, interface::Cli},
    node::EthereumNode,
    pool::TransactionPool,
};
use std::time::Duration;
use tokio::time::sleep;

fn main() {
    Cli::<EthereumChainSpecParser, RethCliTxpoolExt>::parse()
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

    /// Creates a subscription that returns the number of transactions in the pool every 10s.
    #[subscription(name = "subscribeTransactionCount", item = usize)]
    fn subscribe_transaction_count(
        &self,
        #[argument(rename = "delay")] delay: Option<u64>,
    ) -> SubscriptionResult;
}

/// The type that implements the `txpool` rpc namespace trait
pub struct TxpoolExt<Pool> {
    pool: Pool,
}

#[cfg(not(test))]
impl<Pool> TxpoolExtApiServer for TxpoolExt<Pool>
where
    Pool: TransactionPool + Clone + 'static,
{
    fn transaction_count(&self) -> RpcResult<usize> {
        Ok(self.pool.pool_size().total)
    }

    fn subscribe_transaction_count(
        &self,
        pending_subscription_sink: PendingSubscriptionSink,
        delay: Option<u64>,
    ) -> SubscriptionResult {
        let pool = self.pool.clone();
        let delay = delay.unwrap_or(10);
        tokio::spawn(async move {
            let sink = match pending_subscription_sink.accept().await {
                Ok(sink) => sink,
                Err(e) => {
                    println!("failed to accept subscription: {e}");
                    return;
                }
            };

            loop {
                sleep(Duration::from_secs(delay)).await;

                let msg = SubscriptionMessage::from(
                    serde_json::value::to_raw_value(&pool.pool_size().total).expect("serialize"),
                );
                let _ = sink.send(msg).await;
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::{
        http_client::HttpClientBuilder, server::ServerBuilder, ws_client::WsClientBuilder,
    };
    use reth_ethereum::pool::noop::NoopTransactionPool;

    #[cfg(test)]
    impl<Pool> TxpoolExtApiServer for TxpoolExt<Pool>
    where
        Pool: TransactionPool + Clone + 'static,
    {
        fn transaction_count(&self) -> RpcResult<usize> {
            Ok(self.pool.pool_size().total)
        }

        fn subscribe_transaction_count(
            &self,
            pending: PendingSubscriptionSink,
            delay: Option<u64>,
        ) -> SubscriptionResult {
            let delay = delay.unwrap_or(10);
            let pool = self.pool.clone();
            tokio::spawn(async move {
                // Accept the subscription
                let sink = match pending.accept().await {
                    Ok(sink) => sink,
                    Err(err) => {
                        eprintln!("failed to accept subscription: {err}");
                        return;
                    }
                };

                // Send pool size repeatedly, with a 10-second delay
                loop {
                    sleep(Duration::from_millis(delay)).await;
                    let message = SubscriptionMessage::from(
                        serde_json::value::to_raw_value(&pool.pool_size().total)
                            .expect("serialize usize"),
                    );

                    // Just ignore errors if a client has dropped
                    let _ = sink.send(message).await;
                }
            });
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_call_transaction_count_http() {
        let server_addr = start_server().await;
        let uri = format!("http://{server_addr}");
        let client = HttpClientBuilder::default().build(&uri).unwrap();
        let count = TxpoolExtApiClient::transaction_count(&client).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_subscribe_transaction_count_ws() {
        let server_addr = start_server().await;
        let ws_url = format!("ws://{server_addr}");
        let client = WsClientBuilder::default().build(&ws_url).await.unwrap();

        let mut sub = TxpoolExtApiClient::subscribe_transaction_count(&client, None)
            .await
            .expect("failed to subscribe");

        let first = sub.next().await.unwrap().unwrap();
        assert_eq!(first, 0, "expected initial count to be 0");

        let second = sub.next().await.unwrap().unwrap();
        assert_eq!(second, 0, "still expected 0 from our NoopTransactionPool");
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
