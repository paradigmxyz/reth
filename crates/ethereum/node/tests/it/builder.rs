//! Node builder setup tests.

use std::{sync::Arc, time::Duration};

use reth_db::{
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_builder::{
    EngineNodeLauncher, FullNodeComponents, LaunchExecutors, NodeBuilder, NodeConfig,
};
use reth_node_core::{
    args::{DatadirArgs, NetworkArgs},
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_node_ethereum::node::{EthereumAddOns, EthereumNode};
use reth_provider::providers::BlockchainProvider;
use reth_rpc_builder::Identity;
use reth_tasks::{Runtime, RuntimeBuilder, RuntimeConfig};
use tempfile::tempdir;
use tokio::sync::oneshot;

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let msg = "On components".to_string();
    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .on_component_initialized(move |ctx| {
            let _provider = ctx.provider();
            println!("{msg}");
            Ok(())
        })
        .on_node_started(|_full_node| Ok(()))
        .on_rpc_started(|_ctx, handles| {
            let _client = handles.rpc.http_client();
            Ok(())
        })
        .map_add_ons(|addons| addons.with_rpc_middleware(Identity::default()))
        .extend_rpc_modules(|ctx| {
            let _ = ctx.config();
            let _ = ctx.node().provider();

            Ok(())
        })
        .check_launch();
}

#[tokio::test]
async fn test_eth_launcher() {
    let runtime = Runtime::test();
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder =
        NodeBuilder::new(config)
            .with_database(db)
            .with_launch_context(runtime.clone())
            .with_types_and_provider::<EthereumNode, BlockchainProvider<
                NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
            >>()
            .with_components(EthereumNode::components())
            .with_add_ons(EthereumAddOns::default())
            .apply(|builder| {
                let _ = builder.db();
                builder
            })
            .launch_with_fn(|builder| {
                let launcher = EngineNodeLauncher::new(
                    LaunchExecutors::single(runtime.clone()),
                    builder.config().datadir(),
                    Default::default(),
                );
                builder.launch_with(launcher)
            });
}

#[test]
fn test_eth_launcher_with_latency_runtime() {
    let main = RuntimeBuilder::new(RuntimeConfig::default()).build().unwrap();
    let executors = LaunchExecutors::with_latency(main.clone(), 2).unwrap();

    assert!(executors.latency_isolated());
    assert_ne!(main.handle().id(), executors.rpc().handle().id());
    assert_eq!(main.handle().id(), executors.main().handle().id());

    let main_rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let node_handle = main_rt.block_on(async move {
        let tempdir = tempdir().expect("temp datadir");
        let datadir_args = DatadirArgs {
            datadir: MaybePlatformPath::<DataDirPath>::from(tempdir.path().to_path_buf()),
            static_files_path: Some(tempdir.path().join("static")),
            rocksdb_path: Some(tempdir.path().join("rocksdb")),
            pprof_dumps_path: Some(tempdir.path().join("pprof")),
        };
        let mut network = NetworkArgs::default();
        network.discovery.disable_discovery = true;
        let config = NodeConfig::test().with_datadir_args(datadir_args).with_network(network);
        let db = create_test_rw_db();
        let (initialized_tx, initialized_rx) = oneshot::channel();
        let builder =
            NodeBuilder::new(config)
                .with_database(db)
                .with_launch_executors(executors.clone())
                .with_types_and_provider::<EthereumNode, BlockchainProvider<
                    NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                >>()
                .with_components(EthereumNode::components())
                .with_add_ons(EthereumAddOns::default())
                .on_component_initialized(move |node| {
                    assert!(node.latency_isolated());
                    assert_ne!(
                        node.task_executor().handle().id(),
                        node.rpc_task_executor().handle().id(),
                    );
                    let _ = initialized_tx.send(());
                    Ok(())
                })
                .apply(|builder| {
                    let _ = builder.db();
                    builder
                });

        let launcher = EngineNodeLauncher::new(
            executors.clone(),
            builder.config().datadir(),
            Default::default(),
        );
        let launch = builder.launch_with(launcher);
        let node_handle = tokio::time::timeout(Duration::from_secs(30), launch)
            .await
            .expect("timed out waiting for node launch")
            .expect("node launch failed");

        tokio::time::timeout(Duration::from_secs(5), initialized_rx)
            .await
            .expect("timed out waiting for component init hook")
            .expect("component init hook channel dropped");

        node_handle
    });

    node_handle.node.task_executor.graceful_shutdown();
    drop(node_handle);
}

#[test]
fn test_eth_launcher_with_tokio_runtime() {
    // #[tokio::test] can not be used here because we need to create a custom tokio runtime
    // and it would be dropped before the test is finished, resulting in a panic.
    let main_rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let custom_rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    main_rt.block_on(async {
        let runtime = Runtime::test();
        let config = NodeConfig::test();
        let db = create_test_rw_db();
        let _builder =
            NodeBuilder::new(config)
                .with_database(db)
                .with_launch_context(runtime.clone())
                .with_types_and_provider::<EthereumNode, BlockchainProvider<
                    NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                >>()
                .with_components(EthereumNode::components())
                .with_add_ons(
                    EthereumAddOns::default().with_tokio_runtime(Some(custom_rt.handle().clone())),
                )
                .apply(|builder| {
                    let _ = builder.db();
                    builder
                })
                .launch_with_fn(|builder| {
                    let launcher = EngineNodeLauncher::new(
                        LaunchExecutors::single(runtime.clone()),
                        builder.config().datadir(),
                        Default::default(),
                    );
                    builder.launch_with(launcher)
                });
    });
}

#[test]
fn test_node_setup() {
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder =
        NodeBuilder::new(config).with_database(db).node(EthereumNode::default()).check_launch();
}
