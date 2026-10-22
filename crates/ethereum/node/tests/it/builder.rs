//! Node builder setup tests.

use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};

use reth_db::{
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_builder::{
    sync::{BackfillContext, BackfillSyncBuilder, PipelineBackfill},
    EngineNodeLauncher, FullNodeComponents, NodeBuilder, NodeConfig,
};
use reth_node_core::{
    args::DatadirArgs,
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_node_ethereum::node::{EthereumAddOns, EthereumNode};
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    ProviderFactory, StageCheckpointReader,
};
use reth_rpc_builder::Identity;
use reth_stages_types::StageId;
use reth_tasks::Runtime;
use tempfile::{tempdir, TempDir};

// Records the order its hooks run in, building the staged pipeline backfill.
#[derive(Default)]
struct RecordingBackfill {
    // Hooks called so far.
    calls: Arc<Mutex<Vec<&'static str>>>,
    // Whether `recover` refuses the database.
    fail_recovery: bool,
}

impl<N: ProviderNodeTypes, C> BackfillSyncBuilder<N, C> for RecordingBackfill {
    type Backfill = <PipelineBackfill as BackfillSyncBuilder<N, C>>::Backfill;

    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        self.calls.lock().unwrap().push("build");
        PipelineBackfill.build(ctx)
    }

    fn recover(&mut self, provider_factory: &ProviderFactory<N>) -> eyre::Result<()> {
        // The database is open and readable by now.
        provider_factory.provider()?.get_stage_checkpoint(StageId::Finish)?;
        self.calls.lock().unwrap().push("recover");
        if self.fail_recovery {
            eyre::bail!("recovery refused the database")
        }
        Ok(())
    }
}

// A test node config with its own data directory, kept alive by the returned guard.
fn launch_config() -> (NodeConfig<reth_chainspec::ChainSpec>, TempDir) {
    let datadir = tempdir().expect("temp datadir");
    let datadir_args = DatadirArgs {
        datadir: MaybePlatformPath::<DataDirPath>::from_str(datadir.path().to_str().unwrap())
            .expect("valid datadir"),
        static_files_path: Some(datadir.path().join("static")),
        rocksdb_path: Some(datadir.path().join("rocksdb")),
        pprof_dumps_path: Some(datadir.path().join("pprof")),
    };
    (NodeConfig::test().with_datadir_args(datadir_args), datadir)
}

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
                    runtime.clone(),
                    builder.config().datadir(),
                    Default::default(),
                );
                builder.launch_with(launcher)
            });
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
                        runtime.clone(),
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

#[tokio::test(flavor = "multi_thread")]
async fn custom_backfill_recovers_then_builds() -> eyre::Result<()> {
    let (config, _datadir) = launch_config();
    let backfill = RecordingBackfill::default();
    let calls = Arc::clone(&backfill.calls);

    let _node = NodeBuilder::new(config)
        .with_database(create_test_rw_db())
        .with_launch_context(Runtime::test())
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch_with_debug_capabilities_and_backfill(backfill)
        .await?;

    assert_eq!(*calls.lock().unwrap(), ["recover", "build"]);
    Ok(())
}

#[tokio::test]
async fn failed_backfill_recovery_stops_the_launch() {
    let (config, _datadir) = launch_config();
    let backfill = RecordingBackfill { fail_recovery: true, ..Default::default() };
    let calls = Arc::clone(&backfill.calls);

    let launched = NodeBuilder::new(config)
        .with_database(create_test_rw_db())
        .with_launch_context(Runtime::test())
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch_with_debug_capabilities_and_backfill(backfill)
        .await;

    assert!(launched.is_err());
    assert_eq!(*calls.lock().unwrap(), ["recover"]);
}
