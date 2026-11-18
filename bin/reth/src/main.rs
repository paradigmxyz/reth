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
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_ethereum::{snap::EthereumBackfill, EthereumNode};
use tracing::info;

#[cfg(not(target_family = "wasm"))]
use reth_node_pbt::cli::PbtArgs;

fn main() {
    // Ethereum does not use account extensions, so refuse to run a binary that was built with
    // them, for example through feature unification.
    if let Err(err) = reth_primitives_traits::ensure_no_account_extensions() {
        eprintln!("Error: {err}: this binary was built with the `account-ext` feature");
        std::process::exit(1);
    }

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

    reth_node_core::version::try_init_version_metadata(reth_node_core::reth_version_metadata!())
        .expect("version metadata is set once, before anything reads it");

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    #[cfg(not(target_family = "wasm"))]
    let cli = Cli::<EthereumChainSpecParser, PbtArgs>::parse();
    #[cfg(target_family = "wasm")]
    let cli = Cli::<EthereumChainSpecParser>::parse();
    if let Err(err) = cli.run(async move |builder, args| {
        info!(target: "reth::cli", "Launching node");
        let backfill = EthereumBackfill::new(builder.config());
        let node = builder.node(EthereumNode::default());
        #[cfg(not(target_family = "wasm"))]
        let (node, backfill) = {
            let config = args.config()?;
            let backfill = backfill.with_pbt_config(config.clone());
            (node.map_add_ons(|addons| addons.with_pbt_config(config)), backfill)
        };
        #[cfg(target_family = "wasm")]
        let _ = args;
        let handle = node.launch_with_debug_capabilities_and_backfill(backfill).await?;

        handle.wait_for_node_exit().await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
