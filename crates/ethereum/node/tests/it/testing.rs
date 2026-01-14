//! E2E tests for the testing RPC namespace.

use alloy_consensus::TxEnvelope;
use alloy_network::{eip2718::Encodable2718, Ethereum, EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::ExecutionPayloadEnvelopeV4;
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use jsonrpsee_core::client::ClientT;
use reth_db::test_utils::create_test_rw_db;
use reth_e2e_test_utils::wallet::Wallet;
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_node_core::{
    args::DatadirArgs,
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use reth_tasks::TaskManager;
use std::{str::FromStr, sync::Arc};
use tempfile::tempdir;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_works() -> eyre::Result<()> {
    let tasks = TaskManager::current();
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
        .with_launch_context(tasks.executor())
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

/// Helper to test `testing_buildBlockV1` error scenarios with a custom request builder
async fn test_build_block_error<F>(build_request: F) -> eyre::Result<()>
where
    F: FnOnce(Arc<reth_chainspec::ChainSpec>) -> TestingBuildBlockRequestV1 + Send + 'static,
{
    let tasks = TaskManager::current();
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

    let (tx, rx) = oneshot::channel();

    let builder = NodeBuilder::new(config)
        .with_database(db)
        .with_launch_context(tasks.executor())
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .on_rpc_started(move |ctx, handles| {
            let Some(client) = handles.rpc.http_client() else { return Ok(()) };
            let chain = ctx.config().chain.clone();

            tokio::spawn(async move {
                let request = build_request(chain);
                let res: eyre::Result<ExecutionPayloadEnvelopeV4> =
                    client.request("testing_buildBlockV1", [request]).await.map_err(Into::into);
                let _ = tx.send(res);
            });

            Ok(())
        });

    let launcher = builder.engine_api_launcher();
    let _node = builder.launch_with(launcher).await?;

    let res = rx.await.expect("testing_buildBlockV1 response");
    assert!(res.is_err(), "Expected error but got success");

    Ok(())
}

/// Helper to create a signed transaction with custom parameters
fn create_tx(chain_id: u64, gas: u64, nonce: u64) -> Bytes {
    let wallet = Wallet::default().inner;
    let signer = EthereumWallet::from(wallet);

    let tx_request = TransactionRequest {
        nonce: Some(nonce),
        value: Some(U256::from(100)),
        to: Some(TxKind::Call(Address::random())),
        gas: Some(gas),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        chain_id: Some(chain_id),
        input: TransactionInput { input: None, data: None },
        ..Default::default()
    };

    let signed_tx: TxEnvelope = futures::executor::block_on(async {
        <TransactionRequest as TransactionBuilder<Ethereum>>::build(tx_request, &signer)
            .await
            .unwrap()
    });

    Bytes::from(signed_tx.encoded_2718())
}

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_invalid_parent_hash() -> eyre::Result<()> {
    test_build_block_error(|chain| TestingBuildBlockRequestV1 {
        parent_block_hash: B256::from([0xff; 32]), // Invalid parent hash
        payload_attributes: EthPayloadAttributes {
            timestamp: chain.genesis().timestamp + 1,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        },
        transactions: vec![],
        extra_data: None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_invalid_timestamp() -> eyre::Result<()> {
    test_build_block_error(|chain| TestingBuildBlockRequestV1 {
        parent_block_hash: chain.genesis_hash(),
        payload_attributes: EthPayloadAttributes {
            timestamp: chain.genesis().timestamp - 1, // Invalid: earlier than parent
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        },
        transactions: vec![],
        extra_data: None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_tx_insufficient_gas() -> eyre::Result<()> {
    test_build_block_error(|chain| {
        let raw_tx = create_tx(chain.chain.id(), 1000, 0); // Insufficient gas (needs 21000)
        TestingBuildBlockRequestV1 {
            parent_block_hash: chain.genesis_hash(),
            payload_attributes: EthPayloadAttributes {
                timestamp: chain.genesis().timestamp + 1,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: None,
                parent_beacon_block_root: None,
            },
            transactions: vec![raw_tx],
            extra_data: None,
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_tx_invalid_nonce() -> eyre::Result<()> {
    test_build_block_error(|chain| {
        let raw_tx = create_tx(chain.chain.id(), 21000, 10); // Invalid nonce (should be 0)
        TestingBuildBlockRequestV1 {
            parent_block_hash: chain.genesis_hash(),
            payload_attributes: EthPayloadAttributes {
                timestamp: chain.genesis().timestamp + 1,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: None,
                parent_beacon_block_root: None,
            },
            transactions: vec![raw_tx],
            extra_data: None,
        }
    })
    .await
}
