use crate::utils::{eth_payload_attributes, eth_payload_attributes_amsterdam};
use alloy_consensus::Header;
use alloy_eips::Encodable2718;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{bytes, Address, Bytes, TxKind, B256};
use alloy_rpc_types_eth::TransactionRequest;
use jsonrpsee_core::{client::ClientT, rpc_params};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{setup, transaction::TransactionTestContext};
use reth_node_ethereum::EthereumNode;
use reth_provider::StateProviderFactory;
use reth_rpc_api::PayloadStatusWithWitness;
use serde::Deserialize;
use std::sync::Arc;

#[tokio::test]
async fn engine_witness_v4_captures_tree_only_parent() -> eyre::Result<()> {
    check_witness(4).await
}

#[tokio::test]
async fn engine_witness_v5_captures_tree_only_parent() -> eyre::Result<()> {
    check_witness(5).await
}

async fn check_witness(version: u8) -> eyre::Result<()> {
    let contract = Address::with_last_byte(0x80);
    // Read storage without writing it, and access an ancestor block hash.
    let code = bytes!("600054506001405000");
    let mut genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json"))?;
    genesis.alloc.insert(
        contract,
        GenesisAccount {
            code: Some(code.clone()),
            storage: Some([(B256::ZERO, B256::with_last_byte(42))].into()),
            ..Default::default()
        },
    );
    let chain = ChainSpecBuilder::default().chain(MAINNET.chain).genesis(genesis);
    let (chain, attributes) = if version == 5 {
        (chain.amsterdam_activated().build(), eth_payload_attributes_amsterdam as fn(_) -> _)
    } else {
        (chain.prague_activated().build(), eth_payload_attributes as fn(_) -> _)
    };
    let (mut nodes, wallet) = setup::<EthereumNode>(2, Arc::new(chain), false, attributes).await?;
    let target = nodes.pop().unwrap();
    let mut source = nodes.pop().unwrap();
    let first = source.advance_block().await?;
    target.submit_payload(first).await?;
    let parent = source.advance_block().await?;
    let parent_hash = target.submit_payload(parent).await?;
    assert!(target.inner.provider.state_by_block_hash(parent_hash).is_err());

    let tx = TransactionTestContext::sign_tx(
        wallet.inner,
        TransactionRequest {
            to: Some(TxKind::Call(contract)),
            nonce: Some(0),
            chain_id: Some(1),
            gas: Some(100_000),
            max_fee_per_gas: Some(20_000_000_000),
            max_priority_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        },
    )
    .await;
    source.rpc.inject_tx(tx.encoded_2718().into()).await?;
    let child = source.advance_block().await?;
    let block_hash = child.block().hash();
    let (payload, requests) = if version == 5 {
        let envelope = child.try_into_v6()?;
        (serde_json::to_value(envelope.execution_payload)?, envelope.execution_requests)
    } else {
        let envelope = child.try_into_v4()?;
        (
            serde_json::to_value(envelope.envelope_inner.execution_payload)?,
            envelope.execution_requests,
        )
    };
    let client = target.auth_server_handle().http_client();
    let method = format!("engine_newPayloadWithWitnessV{version}");
    let params = rpc_params![&payload, Vec::<B256>::new(), B256::ZERO, &requests];
    let response: PayloadStatusWithWitness = client.request(&method, params.clone()).await?;
    assert!(response.payload_status.is_valid());
    assert_eq!(response.payload_status.latest_valid_hash, Some(block_hash));
    let witness = alloy_rlp::decode_exact::<WireWitness>(&response.witness.unwrap())?;
    assert!(!witness.state.is_empty());
    assert!(witness.codes.contains(&code), "read-only contract code must be captured");
    assert!(witness.state.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(witness.codes.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(witness.keys.is_empty());
    assert_eq!(witness.headers.last().unwrap().hash_slow(), parent_hash);

    // Compare the captured read set with an independent replay on the canonical source node.
    let expected: DebugWitness = source
        .rpc_client()
        .unwrap()
        .request("debug_executionWitnessByBlockHash", rpc_params![block_hash, "canonical"])
        .await?;
    assert_eq!(witness.state, expected.state);
    assert_eq!(witness.codes, expected.codes);
    assert_eq!(
        witness
            .headers
            .iter()
            .map(|header| Bytes::from(alloy_rlp::encode(header)))
            .collect::<Vec<_>>(),
        expected.headers
    );

    let repeated: PayloadStatusWithWitness = client.request(&method, params.clone()).await?;
    assert!(repeated.payload_status.is_valid());
    assert!(repeated.witness.is_none(), "known blocks are not re-executed");
    let ordinary: serde_json::Value =
        client.request(&format!("engine_newPayloadV{version}"), params).await?;
    assert_eq!(ordinary, serde_json::to_value(&repeated.payload_status)?);

    let mut invalid = payload;
    invalid["blockHash"] = serde_json::to_value(B256::ZERO)?;
    let invalid: PayloadStatusWithWitness = client
        .request(&method, rpc_params![invalid, Vec::<B256>::new(), B256::ZERO, requests])
        .await?;
    assert!(invalid.payload_status.is_invalid());
    assert!(invalid.witness.is_none());
    Ok(())
}

#[derive(Debug, alloy_rlp::RlpDecodable)]
struct WireWitness {
    headers: Vec<Header>,
    codes: Vec<Bytes>,
    state: Vec<Bytes>,
    keys: Vec<Bytes>,
}

#[derive(Deserialize)]
struct DebugWitness {
    headers: Vec<Bytes>,
    codes: Vec<Bytes>,
    state: Vec<Bytes>,
}
