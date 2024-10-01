#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use clap::Parser;
use reth::{
    args::{utils::DefaultChainSpecParser, EngineArgs},
    cli::Cli,
};
use reth_node_builder::{engine_tree_config::TreeConfig, EngineNodeLauncher};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_provider::providers::BlockchainProvider2;

fn main() {
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) =
        Cli::<DefaultChainSpecParser, EngineArgs>::parse().run(|builder, engine_args| async move {
            let enable_engine2 = engine_args.experimental;
            match enable_engine2 {
                true => {
                    let engine_tree_config = TreeConfig::default()
                        .with_persistence_threshold(engine_args.persistence_threshold)
                        .with_memory_block_buffer_target(engine_args.memory_block_buffer_target);
                    let handle = builder
                        .with_types_and_provider::<EthereumNode, BlockchainProvider2<_>>()
                        .with_components(EthereumNode::components())
                        .with_add_ons::<EthereumAddOns>()
                        .launch_with_fn(|builder| {
                            let launcher = EngineNodeLauncher::new(
                                builder.task_executor().clone(),
                                builder.config().datadir(),
                                engine_tree_config,
                            );
                            builder.launch_with(launcher)
                        })
                        .await?;
                    handle.node_exit_future.await
                }
                false => {
                    let handle = builder.launch_node(EthereumNode::default()).await?;
                    handle.node_exit_future.await
                }
            }
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
