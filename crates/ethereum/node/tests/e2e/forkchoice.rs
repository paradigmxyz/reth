//! Tests for atomic forkchoice state updates via the Engine API.

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_provider::Provider;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatusEnum};
use jsonrpsee_core::client::Error;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{eth_payload_attributes, test_chain_spec, E2ETestSetupExt};
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use reth_rpc_api::{EngineApiClient, TestingBuildBlockRequestV1};
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};

#[tokio::test]
async fn invalid_forkchoice_preserves_canonical_state() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec(EthereumHardfork::Cancun);
    let (mut nodes, _) = EthereumNode::test_setup(2, chain_spec.clone())
        .with_connect_nodes(false)
        .with_rpc_modifier(|rpc| {
            rpc.with_http_api(RpcModuleSelection::from([
                RethRpcModule::Eth,
                RethRpcModule::Testing,
            ]))
        })
        .build()
        .await?;
    let node = nodes.pop().unwrap();
    let producer = nodes.pop().unwrap();
    let genesis = node.block_hash(0);
    let engine = node.auth_server_handle().http_client();
    let producer_engine = producer.auth_server_handle().http_client();
    let rpc = node.rpc_provider();

    // Build a1 <- a2 <- a3 and b1 <- b2, both rooted at genesis. Only chain A is canonical.
    let mut chains = [vec![genesis], vec![genesis]];
    for (branch, length) in [3, 2].into_iter().enumerate() {
        for number in 1..=length {
            let envelope = producer
                .testing_build_block_v1(TestingBuildBlockRequestV1 {
                    parent_block_hash: *chains[branch].last().unwrap(),
                    payload_attributes: eth_payload_attributes(
                        &chain_spec,
                        number + branch as u64 * 10,
                    ),
                    transactions: vec![],
                    extra_data: None,
                })
                .await?;
            let payload = envelope.execution_payload;
            let hash = payload.payload_inner.payload_inner.block_hash;
            for client in [&producer_engine, &engine] {
                let status = EngineApiClient::<EthEngineTypes>::new_payload_v3(
                    client,
                    payload.clone(),
                    vec![],
                    B256::ZERO,
                )
                .await?;
                assert_eq!(status.status, PayloadStatusEnum::Valid);
            }
            producer.update_forkchoice(genesis, hash).await?;
            chains[branch].push(hash);
        }
        if branch == 0 {
            let status = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v3(
                &engine,
                ForkchoiceState {
                    head_block_hash: chains[0][3],
                    safe_block_hash: genesis,
                    finalized_block_hash: genesis,
                },
                None,
            )
            .await?;
            assert_eq!(status.payload_status.status, PayloadStatusEnum::Valid);
        }
    }
    let [a, b] = chains;

    for (head, safe, finalized) in [
        (b[2], a[2], a[1]),
        (b[2], a[2], b[1]),
        (b[2], b[1], a[1]),
        (b[2], B256::repeat_byte(0xff), b[1]),
        (a[3], b[1], a[1]),
        (a[2], a[3], genesis),
    ] {
        let state = ForkchoiceState {
            head_block_hash: head,
            safe_block_hash: safe,
            finalized_block_hash: finalized,
        };
        let err = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v3(&engine, state, None)
            .await
            .unwrap_err();
        let Error::Call(err) = err else { panic!("Expected an RPC error, got {err:?}") };
        assert_eq!(err.code(), -38002, "{state:?}");

        for (tag, hash) in [
            (BlockNumberOrTag::Latest, a[3]),
            (BlockNumberOrTag::Safe, genesis),
            (BlockNumberOrTag::Finalized, genesis),
            (BlockNumberOrTag::Number(1), a[1]),
            (BlockNumberOrTag::Number(2), a[2]),
            (BlockNumberOrTag::Number(3), a[3]),
        ] {
            assert_eq!(
                rpc.get_block_by_number(tag).await?.unwrap().header.hash,
                hash,
                "{tag:?} changed after rejecting {state:?}"
            );
        }
    }

    // Safe/finalized hashes on the proposed branch must be accepted before it is canonical.
    let status = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v3(
        &engine,
        ForkchoiceState {
            head_block_hash: b[2],
            safe_block_hash: b[2],
            finalized_block_hash: b[1],
        },
        None,
    )
    .await?;
    assert_eq!(status.payload_status.status, PayloadStatusEnum::Valid);
    for (tag, hash) in [
        (BlockNumberOrTag::Latest, b[2]),
        (BlockNumberOrTag::Safe, b[2]),
        (BlockNumberOrTag::Finalized, b[1]),
        (BlockNumberOrTag::Number(1), b[1]),
        (BlockNumberOrTag::Number(2), b[2]),
    ] {
        assert_eq!(rpc.get_block_by_number(tag).await?.unwrap().header.hash, hash);
    }
    assert!(rpc.get_block_by_number(BlockNumberOrTag::Number(3)).await?.is_none());

    // Invalid payload attributes must not roll back an otherwise valid forkchoice update.
    let err = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v3(
        &engine,
        ForkchoiceState::same_hash(b[2]),
        Some(eth_payload_attributes(&chain_spec, 1)),
    )
    .await
    .unwrap_err();
    let Error::Call(err) = err else { panic!("Expected an RPC error, got {err:?}") };
    assert_eq!(err.code(), -38003);
    assert_eq!(
        rpc.get_block_by_number(BlockNumberOrTag::Finalized).await?.unwrap().header.hash,
        b[2]
    );

    Ok(())
}
