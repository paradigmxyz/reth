#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use reth_arbitrum_chainspec::ArbitrumChainSpecParser;
use reth_ethereum_cli::Cli;
use reth_arbitrum_node::{args::RollupArgs, ArbNode};
use tracing::info;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) =
        Cli::<ArbitrumChainSpecParser, RollupArgs>::parse().run(async move |builder, rollup_args| {
            info!(target: "reth::cli", "Launching arb-reth node");
            let handle = builder.node(ArbNode::new(rollup_args)).launch().await?;
            handle.node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
