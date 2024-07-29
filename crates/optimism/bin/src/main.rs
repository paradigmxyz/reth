#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use clap::Parser;
use reth_node_optimism::{args::RollupArgs, rpc::SequencerClient, OptimismNode};
use std::sync::Arc;

// We use jemalloc for performance reasons
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    use reth_optimism_cli::Cli;
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::<RollupArgs>::parse().run(|builder, rollup_args| async move {
        let handle = builder
            .node(OptimismNode::new(rollup_args.clone()))
            .extend_rpc_modules(move |ctx| {
                // register sequencer tx forwarder
                if let Some(sequencer_http) = rollup_args.sequencer_http {
                    ctx.registry.set_eth_raw_transaction_forwarder(Arc::new(SequencerClient::new(
                        sequencer_http,
                    )));
                }

                Ok(())
            })
            .launch()
            .await?;

        handle.node_exit_future.await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
