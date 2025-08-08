#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use reth_cli::Cli;
use reth_arbitrum_chainspec::ArbChainSpec;
use reth_arbitrum_node::{args::RollupArgs, ArbNode};
use tracing::info;

#[derive(Debug, Clone, Default)]
struct ArbChainSpecParser;

impl ChainSpecParser for ArbChainSpecParser {
    type ChainSpec = ArbChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &["dev"];

    fn parse(s: &str) -> eyre::Result<std::sync::Arc<Self::ChainSpec>> {
        if s == "dev" {
            return Ok(std::sync::Arc::new(ArbChainSpec::default()));
        }
        let _genesis = parse_genesis(s)?;
        Ok(std::sync::Arc::new(ArbChainSpec::default()))
    }
}

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) =
        Cli::<ArbChainSpecParser, RollupArgs>::parse().run(async move |builder, rollup_args| {
            info!(target: "arb-reth::cli", "Launching arb-reth");
            let handle = builder.node(ArbNode::new(rollup_args)).launch_with_debug_capabilities().await?;
            handle.node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
