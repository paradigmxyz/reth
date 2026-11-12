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
use reth_node_ethereum::{snap::EthereumBackfill, EthereumNode};
use tracing::info;

#[cfg(feature = "pbt")]
use pbt::{PbtArgs, PbtChainSpecParser};
#[cfg(feature = "pbt")]
use reth_node_pbt::PbtNode;

#[cfg(not(feature = "pbt"))]
use reth_chainspec::ChainSpec;
#[cfg(not(feature = "pbt"))]
use reth_cli::chainspec::ChainSpecParser;
#[cfg(not(feature = "pbt"))]
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
#[cfg(not(feature = "pbt"))]
use std::sync::Arc;

#[cfg(feature = "pbt")]
mod pbt;

#[cfg(not(feature = "pbt"))]
#[derive(Clone, Debug)]
struct StandardChainSpecParser;

#[cfg(not(feature = "pbt"))]
impl ChainSpecParser for StandardChainSpecParser {
    type ChainSpec = ChainSpec;
    const SUPPORTED_CHAINS: &'static [&'static str] = EthereumChainSpecParser::SUPPORTED_CHAINS;

    fn parse(value: &str) -> eyre::Result<Arc<ChainSpec>> {
        let chain = EthereumChainSpecParser::parse(value)?;
        eyre::ensure!(
            !chain
                .genesis
                .config
                .extra_fields
                .get("binaryTrieTime")
                .is_some_and(|value| !value.is_null()),
            "this chain requires a reth binary built with --features pbt"
        );
        Ok(chain)
    }
}

fn validate_pbt_directory(directory: &std::path::Path, enabled: bool) -> eyre::Result<()> {
    eyre::ensure!(
        enabled || !directory.join("pbt/chain-config").try_exists()?,
        "this data directory requires a PBT-enabled binary and --pbt"
    );
    Ok(())
}

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

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    #[cfg(feature = "pbt")]
    let cli = Cli::<PbtChainSpecParser, PbtArgs>::parse();
    #[cfg(not(feature = "pbt"))]
    let cli = Cli::<StandardChainSpecParser>::parse();
    if let Err(err) = cli.run(async move |builder, args| {
        #[cfg(feature = "pbt")]
        let mut builder = builder;
        #[cfg(feature = "pbt")]
        let activation = args.activation(&builder.config().chain)?;
        #[cfg(feature = "pbt")]
        let pbt_enabled = activation.is_some();
        #[cfg(not(feature = "pbt"))]
        let pbt_enabled = {
            let _ = args;
            false
        };
        validate_pbt_directory(builder.config().datadir().data_dir(), pbt_enabled)?;
        info!(target: "reth::cli", "Launching node");
        #[cfg(feature = "pbt")]
        if let Some(activation) = activation {
            let pbt = PbtNode::new_with_config(
                builder.config().chain.clone(),
                builder.config().datadir().data_dir(),
                activation,
                args.config()?,
            )?
            .with_parallel(args.parallel());
            builder.config_mut().chain = pbt.chain_spec();
            let backfill = pbt.backfill();
            let methods = pbt.rpc_module()?;
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components())
                .with_add_ons(pbt.add_ons())
                .extend_rpc_modules(move |ctx| {
                    ctx.modules.remove_method_from_configured("eth_getProof");
                    ctx.auth_module.remove_auth_method("eth_getProof");
                    ctx.auth_module.merge_auth_methods(methods)?;
                    Ok(())
                })
                .launch_with_debug_capabilities_and_backfill(backfill)
                .await?;
            return handle.wait_for_node_exit().await;
        }
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

#[cfg(all(test, not(feature = "pbt")))]
mod standard_chain_tests {
    use super::*;

    #[test]
    fn unsupported_pbt_genesis_is_rejected_before_initialization() {
        let mut genesis = EthereumChainSpecParser::parse("dev").unwrap().genesis.clone();
        genesis.config.extra_fields.insert("binaryTrieTime".into(), serde_json::json!(0));
        let mut file = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        serde_json::to_writer(file.as_file_mut(), &genesis).unwrap();
        assert_eq!(
            StandardChainSpecParser::parse(file.path().to_str().unwrap()).unwrap_err().to_string(),
            "this chain requires a reth binary built with --features pbt"
        );
        let error =
            Cli::<StandardChainSpecParser>::try_parse_from(["reth", "node", "--pbt"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    #[test]
    fn pinned_pbt_data_cannot_be_opened_by_a_standard_node() {
        let directory = tempfile::tempdir().unwrap();
        validate_pbt_directory(directory.path(), false).unwrap();
        std::fs::create_dir(directory.path().join("pbt")).unwrap();
        std::fs::write(directory.path().join("pbt/chain-config"), b"pinned").unwrap();
        assert_eq!(
            validate_pbt_directory(directory.path(), false).unwrap_err().to_string(),
            "this data directory requires a PBT-enabled binary and --pbt"
        );
        validate_pbt_directory(directory.path(), true).unwrap();
    }
}
