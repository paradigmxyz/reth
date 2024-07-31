#![allow(missing_docs)]
#![allow(rustdoc::missing_crate_level_docs)]

// We use jemalloc for performance reasons.
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(not(feature = "optimism"))]
fn main() {
    use reth::cli::Cli;
    use reth_node_ethereum::{launch::EthNodeLauncher, node::EthereumAddOns, EthereumNode};
    use reth_provider::providers::BlockchainProvider2;

    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::parse_args().run(|builder, _| async {
        let handle = builder
            .with_types_and_provider::<EthereumNode, BlockchainProvider2<_>>()
            .with_components(EthereumNode::components())
            .with_add_ons::<EthereumAddOns>()
            .launch_with_fn(|builder| {
                let launcher = EthNodeLauncher::new(
                    builder.task_executor().clone(),
                    builder.config().datadir(),
                );
                builder.launch_with(launcher)
            })
            .await?;
        handle.node_exit_future.await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
