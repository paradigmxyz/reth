#![allow(missing_docs)]

#[global_allocator]
static ALLOC: commonware_allocator::mimalloc::MiMalloc =
    commonware_allocator::mimalloc::MiMalloc::new(commonware_allocator::mimalloc::Config {
        purge: if cfg!(feature = "mimalloc-purge") {
            commonware_allocator::mimalloc::Purge::After(std::time::Duration::from_secs(10))
        } else {
            commonware_allocator::mimalloc::Purge::Default
        },
        reserve: if cfg!(feature = "mimalloc-reserve") {
            Some(commonware_allocator::mimalloc::Reserve {
                bytes: 2 << 30,
                pinned: true,
                prefault: cfg!(feature = "mimalloc-prefault"),
            })
        } else {
            None
        },
        ..commonware_allocator::mimalloc::Config::DEFAULT
    });

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

fn main() {
    let _allocation = Box::new(0u8);
    eprintln!(
        "allocator benchmark: config={:?} status={:?} effective={:?} object_limit={}",
        ALLOC.config(),
        commonware_allocator::mimalloc::Status::read(),
        commonware_allocator::mimalloc::Effective::read(),
        commonware_allocator::mimalloc::LARGE_OBJECT_LIMIT,
    );

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

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    if let Err(err) = Cli::<EthereumChainSpecParser>::parse().run(async move |builder, _| {
        info!(target: "reth::cli", "Launching node");
        let backfill = EthereumBackfill::new(builder.config());
        let handle = builder
            .node(EthereumNode::default())
            .launch_with_debug_capabilities_and_backfill(backfill)
            .await?;

        handle.wait_for_node_exit().await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
