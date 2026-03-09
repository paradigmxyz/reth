//! E2E tests for the testing RPC namespace.

use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::ExecutionPayloadEnvelopeV4;
use jsonrpsee_core::client::ClientT;
use reth_db::test_utils::create_test_rw_db;
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_node_core::{
    args::DatadirArgs,
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use reth_tasks::Runtime;
use std::str::FromStr;
use tempfile::tempdir;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_works() -> eyre::Result<()> {
    let runtime = Runtime::test();
    let mut rpc_args = reth_node_core::args::RpcServerArgs::default().with_http();
    rpc_args.http_api = Some(RpcModuleSelection::from_iter([RethRpcModule::Testing]));
    let tempdir = tempdir().expect("temp datadir");
    let datadir_args = DatadirArgs {
        datadir: MaybePlatformPath::<DataDirPath>::from_str(tempdir.path().to_str().unwrap())
            .expect("valid datadir"),
        static_files_path: Some(tempdir.path().join("static")),
        rocksdb_path: Some(tempdir.path().join("rocksdb")),
        pprof_dumps_path: Some(tempdir.path().join("pprof")),
    };
    let config = NodeConfig::test().with_datadir_args(datadir_args).with_rpc(rpc_args);
    let db = create_test_rw_db();

    let (tx, rx): (
        oneshot::Sender<eyre::Result<ExecutionPayloadEnvelopeV4>>,
        oneshot::Receiver<eyre::Result<ExecutionPayloadEnvelopeV4>>,
    ) = oneshot::channel();

    let builder = NodeBuilder::new(config)
        .with_database(db)
        .with_launch_context(runtime)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .on_rpc_started(move |ctx, handles| {
            let Some(client) = handles.rpc.http_client() else { return Ok(()) };

            let chain = ctx.config().chain.clone();
            let parent_block_hash = chain.genesis_hash();
            let payload_attributes = EthPayloadAttributes {
                timestamp: chain.genesis().timestamp + 1,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: None,
                parent_beacon_block_root: None,
                slot_number: None,
            };

            let request = TestingBuildBlockRequestV1 {
                parent_block_hash,
                payload_attributes,
                transactions: vec![],
                extra_data: None,
            };

            tokio::spawn(async move {
                let res: eyre::Result<ExecutionPayloadEnvelopeV4> =
                    client.request("testing_buildBlockV1", [request]).await.map_err(Into::into);
                let _ = tx.send(res);
            });

            Ok(())
        });

    // Launch the node with the default engine launcher.
    let launcher = builder.engine_api_launcher();
    let _node = builder.launch_with(launcher).await?;

    // Wait for the testing RPC call to return.
    let res = rx.await.expect("testing_buildBlockV1 response");
    assert!(res.is_ok(), "testing_buildBlockV1 failed: {:?}", res.err());

    Ok(())
}
