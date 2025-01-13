//! Scroll binary

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

#[cfg(all(feature = "scroll", not(feature = "optimism")))]
fn main() {
    use clap::Parser;
    use reth_node_builder::{engine_tree_config::TreeConfig, EngineNodeLauncher};
    use reth_provider::providers::BlockchainProvider;
    use reth_scroll_cli::{Cli, ScrollChainSpecParser, ScrollRollupArgs};
    use reth_scroll_node::{ScrollAddOns, ScrollNode};
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::<ScrollChainSpecParser, ScrollRollupArgs>::parse()
        .run::<_, _, ScrollNode>(|builder, _| async move {
            let engine_tree_config = TreeConfig::default()
                .with_persistence_threshold(builder.config().engine.persistence_threshold)
                .with_memory_block_buffer_target(
                    builder.config().engine.memory_block_buffer_target,
                );
            let handle = builder
                .with_types_and_provider::<ScrollNode, BlockchainProvider<_>>()
                .with_components(ScrollNode::components())
                .with_add_ons(ScrollAddOns::default())
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
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

#[cfg(any(feature = "optimism", not(feature = "scroll")))]
fn main() {
    eprintln!("Scroll feature is not enabled");
    std::process::exit(1);
}
