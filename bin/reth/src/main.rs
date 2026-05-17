#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

#[cfg(all(feature = "jemalloc", unix))]
use reth_cli_util::allocator::tikv_jemalloc_sys as _;

#[cfg(all(feature = "jemalloc-prof", unix))]
#[unsafe(export_name = "malloc_conf")]
static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

use alloy_evm::eth::spec::EthExecutorSpec;
use clap::Parser;
use reth::cli::Cli;
use reth_chainspec::{EthereumHardforks, Hardforks};
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm_ethereum::{EthEvmConfig, ExternEvmFactory};
use reth_node_builder::{
    components::ExecutorBuilder, BuilderContext, FullNodeTypes, NodeTypes,
};
use reth_node_ethereum::EthereumNode;
use tracing::info;

/// Executor builder that uses ExternEvmFactory to inject the API_CALL precompile.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
struct ExternEvmExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for ExternEvmExecutorBuilder
where
    Types: NodeTypes<
        ChainSpec: Hardforks + EthExecutorSpec + EthereumHardforks,
        Primitives = EthPrimitives,
    >,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = EthEvmConfig<Types::ChainSpec, ExternEvmFactory>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<Self::EVM> {
        Ok(EthEvmConfig::new_with_evm_factory(
            ctx.chain_spec(),
            ExternEvmFactory::new(),
        ))
    }
}

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    if let Err(err) = Cli::<EthereumChainSpecParser>::parse().run(async move |builder, _| {
        info!(target: "reth::cli", "Launching ExternEVM node");
        info!(target: "reth::cli", "API_CALL precompile at 0x00000000000000000000000000000000000000AA");

        let handle = builder
            .with_types::<reth_node_ethereum::EthereumNode>()
            .with_components(
                EthereumNode::components()
                    .executor(ExternEvmExecutorBuilder),
            )
            .with_add_ons(reth_node_ethereum::EthereumAddOns::default())
            .launch_with_debug_capabilities()
            .await?;

        handle.wait_for_node_exit().await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
