//! E2E tests for the testing RPC namespace.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::BlockNumberOrTag;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{eth_payload_attributes, test_chain_spec, E2ETestSetupExt};
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_json_rpc::client::ClientT;
use reth_node_ethereum::EthereumNode;
use reth_rpc_api::{TestingBuildBlockRequestV1, TestingBuildBlockResponseV1};
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use serde_json::Value;

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_build_block_returns_amsterdam_payload_fields() -> eyre::Result<()> {
    for (fork, is_amsterdam) in
        [(EthereumHardfork::Cancun, false), (EthereumHardfork::Amsterdam, true)]
    {
        let chain_spec = test_chain_spec(fork);
        let (node, _) = EthereumNode::test_setup(1, chain_spec.clone())
            .with_rpc_modifier(|rpc| {
                rpc.with_http_api(RpcModuleSelection::from_iter([RethRpcModule::Testing]))
            })
            .build_single()
            .await?;
        let client = node.rpc_client().expect("HTTP RPC server is enabled");

        let timestamp = chain_spec.genesis().timestamp + 1;
        let slot_number = timestamp + 42;
        let mut payload_attributes = eth_payload_attributes(&chain_spec, timestamp);
        payload_attributes.slot_number = is_amsterdam.then_some(slot_number);
        let parent_beacon_block_root = payload_attributes.parent_beacon_block_root;
        let request = TestingBuildBlockRequestV1 {
            parent_block_hash: chain_spec.genesis_hash(),
            payload_attributes,
            transactions: vec![],
            extra_data: None,
        };
        let response: Value = client.request("testing_buildBlockV1", request.into_params()).await?;
        let payload = &response["executionPayload"];

        assert_eq!(payload.get("blockAccessList").is_some(), is_amsterdam);
        assert_eq!(payload.get("slotNumber").is_some(), is_amsterdam);
        match serde_json::from_value::<TestingBuildBlockResponseV1>(response)? {
            TestingBuildBlockResponseV1::V5(_) => assert!(!is_amsterdam),
            TestingBuildBlockResponseV1::V6(envelope) => {
                assert!(is_amsterdam);
                let payload = envelope.execution_payload;
                assert_eq!(payload.slot_number, slot_number);
                let block_hash = payload.payload_inner.payload_inner.payload_inner.block_hash;
                let mut block = payload.into_block_raw()?;
                block.header.parent_beacon_block_root = parent_beacon_block_root;
                block.header.requests_hash = Some(envelope.execution_requests.requests_hash());
                assert_eq!(block.header.hash_slow(), block_hash);
            }
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn testing_rpc_commit_block_works() -> eyre::Result<()> {
    let chain_spec = test_chain_spec(EthereumHardfork::Amsterdam);
    let (node, _) = EthereumNode::test_setup(1, chain_spec.clone())
        .with_rpc_modifier(|rpc| {
            rpc.with_http_api(RpcModuleSelection::from_iter([
                RethRpcModule::Eth,
                RethRpcModule::Testing,
            ]))
        })
        .build_single()
        .await?;
    let client = node.rpc_client().expect("HTTP RPC server is enabled");

    let timestamp = chain_spec.genesis().timestamp + 1;
    let target_gas_limit = chain_spec.genesis().gas_limit - 100;
    let payload_attributes = EthPayloadAttributes {
        target_gas_limit: Some(target_gas_limit),
        ..eth_payload_attributes(&chain_spec, timestamp)
    };

    let extra_data = Bytes::from_static(b"reth");
    let block_hash: B256 = client
        .request(
            "testing_commitBlockV1",
            (payload_attributes.clone(), Vec::<Bytes>::new(), Some(extra_data.clone())),
        )
        .await?;
    let latest: Value =
        client.request("eth_getBlockByNumber", (BlockNumberOrTag::Latest, false)).await?;
    let latest_hash = latest.get("hash").and_then(Value::as_str).expect("latest block hash");
    assert_eq!(latest_hash, block_hash.to_string());
    assert_eq!(latest.get("extraData"), Some(&serde_json::to_value(extra_data)?));
    assert_eq!(
        latest.get("gasLimit").and_then(Value::as_str),
        Some(format!("{target_gas_limit:#x}").as_str())
    );
    assert!(latest
        .get("transactions")
        .and_then(Value::as_array)
        .expect("block transactions")
        .is_empty());

    let mut next_payload_attributes = payload_attributes;
    next_payload_attributes.timestamp += 12;
    next_payload_attributes.slot_number = Some(next_payload_attributes.timestamp);
    let next_block_hash: B256 = client
        .request(
            "testing_commitBlockV1",
            (next_payload_attributes, Option::<Vec<Bytes>>::None, Option::<Bytes>::None),
        )
        .await?;
    let next_latest: Value =
        client.request("eth_getBlockByNumber", (BlockNumberOrTag::Latest, false)).await?;
    let next_block_hash = next_block_hash.to_string();
    let block_hash = block_hash.to_string();
    assert_eq!(next_latest.get("hash").and_then(Value::as_str), Some(next_block_hash.as_str()));
    assert_eq!(next_latest.get("parentHash").and_then(Value::as_str), Some(block_hash.as_str()));

    Ok(())
}
