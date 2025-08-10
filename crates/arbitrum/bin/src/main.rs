#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use reth_cli_runner::CliRunner;
use reth_cli_commands::launcher::FnLauncher;
use reth_cli::Cli as EthCli;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use reth_db::DatabaseEnv;
use std::sync::Arc;
use tracing::info;

use reth_arbitrum_node::{args::RollupArgs, ArbNode};

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = EthCli::<reth_cli::chainspec::ChainSpecParser, RollupArgs>::parse().with_runner(
        CliRunner::try_default_runtime().expect("runtime"),
        |builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, reth_chainspec::ChainSpec>>, rollup_args: RollupArgs| async move {
            info!(target: "reth::cli", "Launching arb-reth node");
            let handle = builder.node(ArbNode::new(rollup_args)).launch_with_debug_capabilities().await?;
            handle.node_exit_future.await
        },
    ) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
