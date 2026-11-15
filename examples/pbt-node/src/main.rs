//! Dev node scheduling binary-tree commitments through standard Ethereum setup.

use reth_ethereum::{
    node::{
        builder::NodeBuilder,
        core::{
            args::{DatadirArgs, RpcServerArgs},
            node_config::NodeConfig,
        },
        EthereumNode,
    },
    tasks::Runtime,
};
use reth_node_pbt::configure_chain;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let configured_directory = std::env::var_os("PBT_DATADIR");
    let temporary = configured_directory.is_none().then(tempfile::tempdir).transpose()?;
    let directory = configured_directory
        .map(PathBuf::from)
        .or_else(|| temporary.as_ref().map(|directory| directory.path().to_path_buf()))
        .ok_or_else(|| eyre::eyre!("missing PBT data directory"))?;
    let activation = match std::env::var("PBT_ACTIVATION_TIMESTAMP") {
        Ok(value) => value.parse::<u64>()?,
        Err(std::env::VarError::NotPresent) => 0,
        Err(error) => return Err(error.into()),
    };
    let mut chain =
        reth_ethereum::chainspec::ChainSpecBuilder::from(&*reth_ethereum::chainspec::DEV)
            .bogota_activated()
            .build();
    chain.genesis.config.extra_fields.insert("binaryTrieTime".into(), activation.into());
    let chain = configure_chain(Arc::new(chain))?;
    let config = NodeConfig::test()
        .dev()
        .with_datadir_args(DatadirArgs { datadir: directory.into(), ..Default::default() })
        .with_rpc(RpcServerArgs::default().with_http())
        .with_chain(chain);
    let database = Arc::new(reth_ethereum::provider::db::init_db(
        config.datadir().db(),
        reth_ethereum::provider::db::mdbx::DatabaseArguments::default(),
    )?);
    let runtime = Runtime::test();
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let backfill = reth_ethereum::node::snap::EthereumBackfill::new(&config);
    let mut handle = NodeBuilder::new(config)
        .with_database(database)
        .with_launch_context(runtime.clone())
        .node(EthereumNode::default())
        .launch_with_debug_capabilities_and_backfill(backfill)
        .await?;
    println!(
        "PBT dev node running; HTTP RPC: {:?}; authenticated RPC: {}",
        handle.node.rpc_server_handles.rpc.http_local_addr(),
        handle.node.auth_server_handle().http_url()
    );
    let result = tokio::select! {
        result = &mut handle.node_exit_future => result,
        _ = interrupt.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
    };
    let rpc_stopped = handle.node.rpc_server_handle().clone().stop();
    let auth_stopped = handle.node.auth_server_handle().clone().stop();
    let stopped = tokio::task::spawn_blocking(move || {
        runtime.graceful_shutdown_with_timeout(Duration::from_secs(30))
    })
    .await?;
    eyre::ensure!(stopped, "PBT dev node did not finish shutdown within 30 seconds");
    result?;
    rpc_stopped?;
    auth_stopped?;
    Ok(())
}
