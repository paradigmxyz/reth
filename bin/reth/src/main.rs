#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use clap::Parser;
use reth::{
    cli::Cli, ress::install_ress_subprotocol, zk_ress::install_zk_ress_subprotocol, ExtraArgs,
};
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_builder::NodeHandle;
use reth_node_ethereum::EthereumNode;
use reth_ress_provider::{maintain_pending_state, PendingState, RethRessProtocolProvider};
use reth_zk_ress_provider::ZkRessProver;
use tracing::info;

fn main() {
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    if let Err(err) =
        Cli::<EthereumChainSpecParser, ExtraArgs>::parse().run(async move |builder, extra_args| {
            info!(target: "reth::cli", "Launching node");
            let NodeHandle { node, node_exit_future } =
                builder.node(EthereumNode::default()).launch_with_debug_capabilities().await?;

            // Install ress & zkress subprotocols.
            if extra_args.ress.enabled || !extra_args.zk_ress.protocols.is_empty() {
                info!(target: "reth::cli", "Initializing ress pending state");
                let pending_state = PendingState::default();

                // Spawn maintenance task for pending state.
                node.task_executor.spawn(maintain_pending_state(
                    node.add_ons_handle.engine_events.new_listener(),
                    node.provider.clone(),
                    pending_state.clone(),
                ));

                let provider = RethRessProtocolProvider::new(
                    node.provider.clone(),
                    node.evm_config.clone(),
                    Box::new(node.task_executor.clone()),
                    extra_args.ress.max_witness_window,
                    extra_args.ress.witness_max_parallel,
                    extra_args.ress.witness_cache_size,
                    pending_state,
                )?;

                // Install ress
                if extra_args.ress.enabled {
                    install_ress_subprotocol(
                        provider.clone(),
                        node.network.clone(),
                        node.task_executor.clone(),
                        extra_args.ress.max_active_connections,
                    )?;
                }

                // Install zk ress
                for protocol in &extra_args.zk_ress.protocols {
                    let prover = ZkRessProver::try_from_arg(protocol)?;
                    install_zk_ress_subprotocol(
                        prover,
                        0,
                        provider.clone(),
                        node.network.clone(),
                        node.task_executor.clone(),
                        extra_args.ress.max_active_connections,
                    )?;
                }
            }

            node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
