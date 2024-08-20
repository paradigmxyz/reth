#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use reth::cli::Cli;
use reth_node_builder::EngineNodeLauncher;
use reth_node_optimism::{
    args::RollupArgs, node::OptimismAddOns, rpc::SequencerClient, OptimismNode,
};
use reth_provider::providers::BlockchainProvider2;
use std::sync::Arc;

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

                        Ok(())
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

                        Ok(())
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
