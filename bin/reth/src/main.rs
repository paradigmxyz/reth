#![allow(missing_docs)]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

// Required for "override_allocator_on_supported_platforms".
#[cfg(all(feature = "jemalloc", unix))]
use reth_cli_util::allocator::tikv_jemalloc_sys as _;

#[cfg(all(feature = "jemalloc-prof", unix))]
#[unsafe(export_name = "malloc_conf")]
static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

use clap::Parser;
use reth::cli::Cli;
use reth_db::mdbx::SyncMode;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_ethereum::EthereumNode;
use tracing::info;

fn default_node_sync_mode(cli: &mut Cli<EthereumChainSpecParser>) {
    cli.apply_node_command(|command| {
        command.db.sync_mode.get_or_insert(SyncMode::SafeNoSync);
    });
}

fn main() {
    #[cfg(feature = "jit")]
    {
        match reth_node_ethereum::node::maybe_run_jit_helper() {
            Ok(std::ops::ControlFlow::Break(())) => return,
            Ok(std::ops::ControlFlow::Continue(())) => {}
            Err(err) => {
                eprintln!("Error: {err:?}");
                std::process::exit(1);
            }
        }
    }

    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    let mut cli = Cli::<EthereumChainSpecParser>::parse();
    default_node_sync_mode(&mut cli);

    if let Err(err) = cli.run(async move |builder, _| {
        info!(target: "reth::cli", "Launching node");
        let handle = builder.node(EthereumNode::default()).launch_with_debug_capabilities().await?;

        handle.wait_for_node_exit().await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_defaults_to_safe_no_sync() {
        let mut cli = Cli::<EthereumChainSpecParser>::parse_from(["reth", "node"]);
        default_node_sync_mode(&mut cli);

        let command = cli.as_node_command_mut().expect("node command");
        assert!(matches!(command.db.sync_mode, Some(SyncMode::SafeNoSync)));
    }

    #[test]
    fn explicit_node_sync_mode_is_preserved() {
        let mut cli = Cli::<EthereumChainSpecParser>::parse_from([
            "reth",
            "node",
            "--db.sync-mode",
            "durable",
        ]);
        default_node_sync_mode(&mut cli);

        let command = cli.as_node_command_mut().expect("node command");
        assert!(matches!(command.db.sync_mode, Some(SyncMode::Durable)));
    }
}
