#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use async_trait::async_trait;
use clap::Parser;
use futures::StreamExt;
use jsonrpsee::{
    core::RpcResult,
    proc_macros::rpc,
    types::{error::INTERNAL_ERROR_CODE, ErrorObject, ErrorObjectOwned},
};
use reth::cli::Cli;
use reth_exex::{ExExContext, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_tracing::tracing::info;
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use reth_node_builder::EngineNodeLauncher;
use reth_node_optimism::{
    args::RollupArgs, node::OptimismAddOns, rpc::SequencerClient, OptimismNode,
};
use reth_provider::providers::BlockchainProvider2;

// We use jemalloc for performance reasons
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(not(feature = "optimism"))]
compile_error!("Cannot build the `op-reth` binary with the `optimism` feature flag disabled. Did you mean to build `reth`?");

#[cfg(feature = "optimism")]
fn main() {
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::<RollupArgs>::parse().run(|builder, rollup_args| async move {
        let (rpc_tx, rpc_rx) = mpsc::unbounded_channel(); // For WallTimeExEx

        let enable_engine2 = rollup_args.experimental;
        let sequencer_http_arg = rollup_args.sequencer_http.clone();
        match enable_engine2 {
            true => {
                let handle = builder
                    .with_types_and_provider::<OptimismNode, BlockchainProvider2<_>>()
                    .with_components(OptimismNode::components(rollup_args))
                    .with_add_ons::<OptimismAddOns>()
                    .extend_rpc_modules(move |ctx| {
                        // register sequencer tx forwarder
                        if let Some(sequencer_http) = sequencer_http_arg {
                            ctx.registry.set_eth_raw_transaction_forwarder(Arc::new(
                                SequencerClient::new(sequencer_http),
                            ));
                        }

                        ctx.modules.merge_configured(WallTimeRpcExt { to_exex: rpc_tx }.into_rpc())?;

                        Ok(())
                    })
                    .install_exex("walltime", |ctx| async move {
                        Ok(WallTimeExEx::new(ctx, UnboundedReceiverStream::from(rpc_rx)))
                    })
                    .launch_with_fn(|builder| {
                        let launcher = EngineNodeLauncher::new(
                            builder.task_executor().clone(),
                            builder.config().datadir(),
                        );
                        builder.launch_with(launcher)
                    })
                    .await?;

                handle.node_exit_future.await
            }
            false => {
                let handle = builder
                    .node(OptimismNode::new(rollup_args.clone()))
                    .extend_rpc_modules(move |ctx| {
                        // register sequencer tx forwarder
                        if let Some(sequencer_http) = sequencer_http_arg {
                            ctx.registry.set_eth_raw_transaction_forwarder(Arc::new(
                                SequencerClient::new(sequencer_http),
                            ));
                        }

                        ctx.modules.merge_configured(WallTimeRpcExt { to_exex: rpc_tx }.into_rpc())?;

                        Ok(())
                    })
                    .install_exex("walltime", |ctx| async move {
                        Ok(WallTimeExEx::new(ctx, UnboundedReceiverStream::from(rpc_rx)))
                    })
                    .launch()
                    .await?;

                handle.node_exit_future.await
            }
        }
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

pub fn unix_epoch_ms() -> u64 {
    use std::time::SystemTime;
    let now = SystemTime::now();
    now.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|err| panic!("Current time {now:?} is invalid: {err:?}"))
        .as_millis() as u64
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlockTimeData {
    /// Wall time of last block
    wall_time_ms: u64,
    /// Timestamp of last block (chain time)
    block_timestamp: u64,
}

#[derive(Debug)]
pub struct WallTimeExEx<Node: FullNodeComponents> {
    /// The context of the `ExEx`
    ctx: ExExContext<Node>,
    /// Incoming RPC requests.
    rpc_requests_stream: UnboundedReceiverStream<oneshot::Sender<WallTimeData>>,
    /// Time data of last block
    last_block_timedata: BlockTimeData,
}

impl<Node: FullNodeComponents> WallTimeExEx<Node> {
    fn new(
        ctx: ExExContext<Node>,
        rpc_requests_stream: UnboundedReceiverStream<oneshot::Sender<WallTimeData>>,
    ) -> Self {
        Self { ctx, rpc_requests_stream, last_block_timedata: BlockTimeData::default() }
    }
}

impl<Node: FullNodeComponents + Unpin> Future for WallTimeExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            if let Poll::Ready(Some(notification)) = this.ctx.notifications.poll_recv(cx) {
                match &notification {
                    ExExNotification::ChainCommitted { new } => {
                        info!(committed_chain = ?new.range(), "Received commit");
                    }
                    ExExNotification::ChainReorged { old, new } => {
                        info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
                    }
                    ExExNotification::ChainReverted { old } => {
                        info!(reverted_chain = ?old.range(), "Received revert");
                    }
                };

                if let Some(committed_chain) = notification.committed_chain() {
                    this.last_block_timedata.block_timestamp = committed_chain.tip().timestamp;
                    this.last_block_timedata.wall_time_ms = unix_epoch_ms();
                }
                continue;
            }

            if let Poll::Ready(Some(tx)) = this.rpc_requests_stream.poll_next_unpin(cx) {
                let _ = tx.send(WallTimeData {
                    current_wall_time_ms: unix_epoch_ms(),
                    last_block_wall_time_ms: this.last_block_timedata.wall_time_ms.clone(),
                    last_block_timestamp: this.last_block_timedata.block_timestamp.clone(),
                });
                continue;
            }

            return Poll::Pending;
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WallTimeData {
    /// Wall time right now
    current_wall_time_ms: u64,
    /// Wall time of last block
    last_block_wall_time_ms: u64,
    /// Timestamp of last block (chain time)
    last_block_timestamp: u64,
}

#[cfg_attr(not(test), rpc(server, namespace = "ext"))]
#[cfg_attr(test, rpc(server, client, namespace = "ext"))]
trait WallTimeRpcExtApi {
    /// Return the wall time and block timestamp of the latest block.
    #[method(name = "getWallTimeData")]
    async fn get_timedata(&self) -> RpcResult<WallTimeData>;
}

#[derive(Debug)]
pub struct WallTimeRpcExt {
    to_exex: mpsc::UnboundedSender<oneshot::Sender<WallTimeData>>,
}

#[async_trait]
impl WallTimeRpcExtApiServer for WallTimeRpcExt {
    async fn get_timedata(&self) -> RpcResult<WallTimeData> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_exex.send(tx).map_err(|_| rpc_internal_error())?;
        rx.await.map_err(|_| rpc_internal_error())
    }
}

#[inline]
fn rpc_internal_error() -> ErrorObjectOwned {
    ErrorObject::owned(INTERNAL_ERROR_CODE, "internal error", Some(""))
}
