#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use clap::Parser;
use reth::{args::RessArgs, ress::install_ress_subprotocol};
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_builder::NodeHandle;
use tracing::info;

fn main() {
    use reth::cli::Cli;
    use reth::commands::bitfinity_import::BitfinityImportCommand;
    use reth_node_ethereum::EthereumNode;

    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    if let Err(err) =
        Cli::<EthereumChainSpecParser, RessArgs>::parse().run(async move |builder, ress_args| {
            info!(target: "reth::cli", "Launching node");
            let NodeHandle { node, node_exit_future, bitfinity_import } =
                builder.node(EthereumNode::default()).launch_with_debug_capabilities().await?;

            {
                let blockchain_provider = node.provider.clone();
                let config = node.config.config.clone();
                let chain = node.chain_spec();
                let datadir = node.data_dir.clone();
                let (provider_factory, bitfinity) = bitfinity_import.clone().expect("Bitfinity import not configured");                    

                // Init bitfinity import
                let import = BitfinityImportCommand::new(
                    config,
                    datadir,
                    chain,
                    bitfinity.clone(),
                    provider_factory,
                    blockchain_provider,
                );
                let _import_handle = import.schedule_execution().await?;
			}

            // Install ress subprotocol.
            if ress_args.enabled {
                install_ress_subprotocol(
                    ress_args,
                    node.provider,
                    node.block_executor,
                    node.network,
                    node.task_executor,
                    node.add_ons_handle.engine_events.new_listener(),
                )?;
            }
            
            node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
