#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use reth_apollo::{ApolloConfig, ApolloService};
use reth_optimism_cli::{chainspec::OpChainSpecParser, Cli};
use reth_optimism_node::{args::RollupArgs, OpNode};
use tracing::info;

use std::{path::Path, sync::Arc};
use tracing::error;
use xlayer_db::utils::initialize;
use xlayer_exex::utils::post_exec_exex;
use xlayer_rpc::utils::{XlayerExt, XlayerExtApiServer};

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe {
            std::env::set_var("RUST_BACKTRACE", "1");
        }
    }

    if let Err(err) =
        Cli::<OpChainSpecParser, RollupArgs>::parse().run(async move |builder, rollup_args| {
            info!(target: "reth::cli", "Launching node");

            // For X Layer
            if rollup_args.xlayer_args.apollo.enabled {
                run_apollo(&rollup_args).await;
            }

            let enable_inner_tx = rollup_args.xlayer_args.enable_inner_tx;
            let data_dir = builder.config().datadir();
            let mut node_builder = builder.node(OpNode::new(rollup_args));

            if enable_inner_tx {
                // Conditionally initialize InnerTx database before consuming builder
                let db_path = data_dir.db().parent().unwrap_or_else(|| Path::new("/")).to_path_buf();
                match initialize(db_path) {
                    Ok(_) => info!(target: "reth::cli", "xlayer db initialized"),
                    Err(e) => error!(target: "reth::cli", "xlayer db failed to initialize {:#?}", e),
                }

                node_builder = node_builder
                    .extend_rpc_modules(move |ctx| {
                        let new_op_eth_api = ctx.registry.eth_api().clone();
                        let custom_rpc = XlayerExt { backend: Arc::new(new_op_eth_api) };
                        ctx.modules.merge_configured(custom_rpc.into_rpc())?;
                        info!(target:"reth::cli", "xlayer innertx rpc enabled");
                        Ok(())
                    })
                    .install_exex("post_exec_exex", |ctx| async move { Ok(post_exec_exex(ctx)) });
            }

            let handle = node_builder.launch_with_debug_capabilities().await?;

            handle.node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

async fn run_apollo(rollup_args: &RollupArgs) {
    tracing::info!(target: "reth::apollo", "[Apollo] Apollo enabled: {:?}", rollup_args.xlayer_args.apollo.enabled);
    tracing::info!(target: "reth::apollo", "[Apollo] Apollo app ID: {:?}", rollup_args.xlayer_args.apollo.apollo_app_id);
    tracing::info!(target: "reth::apollo", "[Apollo] Apollo IP: {:?}", rollup_args.xlayer_args.apollo.apollo_ip);
    tracing::info!(target: "reth::apollo", "[Apollo] Apollo cluster: {:?}", rollup_args.xlayer_args.apollo.apollo_cluster);
    tracing::info!(target: "reth::apollo", "[Apollo] Apollo namespace: {:?}", rollup_args.xlayer_args.apollo.apollo_namespace);

    // Create Apollo config from args
    let apollo_config = ApolloConfig {
        meta_server: vec![rollup_args.xlayer_args.apollo.apollo_ip.to_string()],
        app_id: rollup_args.xlayer_args.apollo.apollo_app_id.to_string(),
        cluster_name: rollup_args.xlayer_args.apollo.apollo_cluster.to_string(),
        namespaces: Some(rollup_args.xlayer_args.apollo.apollo_namespace.split(',').map(|s| s.to_string()).collect()),
        secret: None,
    };

    tracing::info!(target: "reth::apollo", "[Apollo] Creating Apollo config");

    // Initialize Apollo singleton
    if let Err(e) = ApolloService::try_initialize(apollo_config).await {
        tracing::error!(target: "reth::apollo", "[Apollo] Failed to initialize Apollo: {:?}; Proceeding with node launch without Apollo", e);
    } else {
        tracing::info!(target: "reth::apollo", "[Apollo] Apollo initialized successfully")
    }
}
