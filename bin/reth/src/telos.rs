#![allow(missing_docs)]

use clap::Parser;
use reth::cli::Cli;
use reth_node_telos::{TelosArgs, TelosNode};
use reth_telos_rpc::TelosClient;

// We use jemalloc for performance reasons.
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "optimism", not(test)))]
compile_error!("Cannot build the `telos-reth` binary with the `optimism` feature flag enabled. Did you mean to build `op-reth`?");

#[cfg(feature = "telos")]
fn main() {
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::<TelosArgs>::parse().run(|builder, telos_args| async move {
        let handle = builder
            .node(TelosNode::new(telos_args.clone()))
            .extend_rpc_modules(move |ctx| {
                if telos_args.telos_endpoint.is_some() {
                    ctx.registry
                        .eth_api()
                        .set_telos_client(TelosClient::new(telos_args.into()));
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
