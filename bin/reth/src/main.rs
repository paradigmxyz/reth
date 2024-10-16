#![allow(missing_docs)]

// We use jemalloc for performance reasons.
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "optimism", not(test)))]
compile_error!("Cannot build the `reth` binary with the `optimism` feature flag enabled. Did you mean to build `op-reth`?");

#[cfg(not(feature = "optimism"))]
fn main() {
    use reth::cli::Cli;
    use reth::commands::bitfinity_import::BitfinityImportCommand;
    use reth_node_ethereum::EthereumNode;

    reth::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::parse_args().run(|builder, _| async {
        let handle = builder.launch_node(EthereumNode::default()).await?;

        let blockchain_provider = handle.node.provider.clone();
        let config = handle.node.config.config.clone();
        let chain = handle.node.chain_spec();
        let datadir = handle.node.data_dir.clone();
        let (provider_factory, bitfinity) =
            handle.bitfinity_import.clone().expect("Bitfinity import not configured");

        // Init bitfinity import
        {
            let import = BitfinityImportCommand::new(
                config,
                datadir,
                chain,
                bitfinity,
                provider_factory,
                blockchain_provider,
            );
            let _import_handle = import.schedule_execution().await?;
        }

        handle.node_exit_future.await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
