//! End-to-end EIP-8141 pool coverage using externally submitted frame envelopes.

use crate::utils::eth_payload_attributes_amsterdam;
use alloy_consensus::{BlockHeader, TxEip8141};
use alloy_eips::eip8141::{
    Frame, FrameAddress, FrameLimits, FrameMode, FrameSignature, SignatureScheme, TransactionFees,
    ATOMIC_BATCH_FLAG, EXPIRY_VERIFIER,
};
use alloy_primitives::{Address, Bytes, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{setup_engine, wallet::Wallet};
use reth_node_ethereum::EthereumNode;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

const VERIFY_GAS: u64 = 10_000;
const USER_OP_GAS: u64 = 30_000;
const MAX_FEE_PER_GAS: u64 = 20_000_000_000;
const MAX_PRIORITY_FEE_PER_GAS: u64 = 2_000_000_000;
const fn recipient() -> Address {
    Address::repeat_byte(0x11)
}

fn chain_spec() -> Arc<reth_chainspec::ChainSpec> {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            // The Frames devnet aliases Bogotá to Amsterdam. The E2E chain enables both so the
            // pool gate and the V6 payload path exercise the same configuration.
            .bogota_activated()
            .build(),
    )
}

fn self_verify_frame() -> Frame {
    Frame {
        mode: FrameMode::Verify,
        flags: 3,
        limits: FrameLimits { execution: VERIFY_GAS, state: 0 },
        ..Default::default()
    }
}

fn sender_frame(target: Address) -> Frame {
    Frame {
        mode: FrameMode::Sender,
        target: FrameAddress::from(target),
        limits: FrameLimits { execution: USER_OP_GAS, state: 0 },
        ..Default::default()
    }
}

/// Builds an EIP-8141 envelope using the `v || r || s` SEC256K1 signature encoding.
fn frame_tx(signer: &PrivateKeySigner, nonce: u64, frames: Vec<Frame>) -> Bytes {
    let mut tx = TxEip8141 {
        chain_id: 1,
        nonce,
        nonce_keys: Some(vec![U256::ZERO]),
        sender: signer.address(),
        frames,
        signatures: vec![FrameSignature {
            scheme: SignatureScheme::Secp256k1,
            ..Default::default()
        }],
        fees: TransactionFees {
            max_priority_fee_per_gas: U256::from(MAX_PRIORITY_FEE_PER_GAS),
            max_fee_per_gas: U256::from(MAX_FEE_PER_GAS),
            max_fee_per_blob_gas: U256::ZERO,
        },
        ..Default::default()
    };
    let signature = signer.sign_hash_sync(&tx.signature_hash()).unwrap();
    let mut frame_signature = Vec::with_capacity(65);
    // EIP-8141 encodes secp256k1 signatures as `v || r || s`, with `v` as
    // the raw recovery bit. Alloy's byte representation uses Ethereum's
    // legacy `27/28` notation, so construct the frame encoding explicitly.
    frame_signature.push(u8::from(signature.v()));
    frame_signature.extend_from_slice(&signature.r().to_be_bytes::<32>());
    frame_signature.extend_from_slice(&signature.s().to_be_bytes::<32>());
    tx.signatures[0].signature = frame_signature.into();

    let mut raw = Vec::with_capacity(tx.eip2718_encoded_length());
    tx.eip2718_encode(&mut raw);
    eprintln!(
        "EIP-8141 E2E envelope: hash={:#x}, sender={:#x}, nonce={}, frames={:?}, signature={:#x}",
        tx.tx_hash(),
        tx.sender,
        tx.nonce,
        tx.frames,
        tx.signatures[0].signature,
    );
    raw.into()
}

async fn assert_mined_from_pool(
    node: &mut reth_e2e_test_utils::NodeHelperType<EthereumNode>,
    expected: &[alloy_primitives::B256],
) -> eyre::Result<()> {
    for hash in expected {
        assert!(node.inner.pool.contains(hash), "frame transaction was not admitted to the pool");
        eprintln!("EIP-8141 E2E pool admission confirmed: hash={hash:#x}");
    }

    let payload = node.new_payload().await?;
    let hashes = payload.block().body().transactions().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
    for hash in expected {
        assert!(hashes.contains(hash), "frame transaction was not selected for the payload");
        eprintln!("EIP-8141 E2E payload inclusion confirmed: hash={hash:#x}");
    }

    let block_number = payload.block().number();
    let block_hash = node.submit_payload(payload).await?;
    node.update_forkchoice(block_hash, block_hash).await?;

    // The transaction pool is maintained by a separate task subscribed to the canonical-state
    // stream. Wait until the block is persisted before checking that task's asynchronous removal.
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        node.wait_block(block_number, block_hash, false),
    )
    .await
    .map_err(|_| eyre::eyre!("timed out waiting for the frame block to become canonical"))??;

    let removed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if expected.iter().all(|hash| !node.inner.pool.contains(hash)) {
                break true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or(false);
    eyre::ensure!(removed, "canonical frame transaction remained in pool");
    for hash in expected {
        eprintln!("EIP-8141 E2E canonical removal confirmed: hash={hash:#x}");
    }
    Ok(())
}

#[tokio::test]
async fn self_verify_frame_is_admitted_and_mined() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let signer = Wallet::default().wallet_gen().into_iter().next().unwrap();
    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        chain_spec(),
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let raw = frame_tx(&signer, 0, vec![self_verify_frame(), sender_frame(recipient())]);
    let hash = node.rpc.inject_tx(raw).await?;

    assert_mined_from_pool(&mut node, &[hash]).await
}

#[tokio::test]
async fn expiry_prefix_frame_is_admitted_and_mined() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let signer = Wallet::default().wallet_gen().into_iter().next().unwrap();
    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        chain_spec(),
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let deadline = node.payload.timestamp.saturating_add(600).to_be_bytes();
    let expiry = Frame {
        mode: FrameMode::Verify,
        target: FrameAddress::from(EXPIRY_VERIFIER),
        limits: FrameLimits { execution: VERIFY_GAS, state: 0 },
        data: Bytes::copy_from_slice(&deadline),
        ..Default::default()
    };
    let raw = frame_tx(&signer, 0, vec![expiry, self_verify_frame(), sender_frame(recipient())]);
    let hash = node.rpc.inject_tx(raw).await?;

    assert_mined_from_pool(&mut node, &[hash]).await
}

#[tokio::test]
async fn atomic_frame_body_is_admitted_and_mined() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let signer = Wallet::default().wallet_gen().into_iter().next().unwrap();
    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        chain_spec(),
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let mut first = sender_frame(recipient());
    first.flags = ATOMIC_BATCH_FLAG;
    let mut second = sender_frame(recipient());
    second.flags = ATOMIC_BATCH_FLAG;
    let raw =
        frame_tx(&signer, 0, vec![self_verify_frame(), first, second, sender_frame(recipient())]);
    let hash = node.rpc.inject_tx(raw).await?;

    assert_mined_from_pool(&mut node, &[hash]).await
}

#[tokio::test]
async fn sequential_frames_wait_for_the_current_sequence() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let signer = Wallet::default().wallet_gen().into_iter().next().unwrap();
    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        chain_spec(),
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let first = node
        .rpc
        .inject_tx(frame_tx(&signer, 0, vec![self_verify_frame(), sender_frame(recipient())]))
        .await?;
    let second_raw = frame_tx(&signer, 1, vec![self_verify_frame(), sender_frame(recipient())]);
    assert!(node.rpc.inject_tx(second_raw.clone()).await.is_err());
    assert_mined_from_pool(&mut node, &[first]).await?;
    let second = node.rpc.inject_tx(second_raw).await?;
    assert_mined_from_pool(&mut node, &[second]).await
}

#[tokio::test]
async fn keyed_nonce_rpc_and_pool_roundtrip() -> eyre::Result<()> {
    use alloy_eips::{
        eip8141::{nonce_slot, NONCE_MANAGER},
        Decodable2718,
    };
    use serde_json::json;
    let signer = Wallet::default().wallet_gen().into_iter().next().unwrap();
    let spec = chain_spec();
    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        spec,
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let client = reqwest::Client::new();
    let url = node.rpc_url();
    let rpc = |method: &'static str, params: serde_json::Value| {
        let client = &client;
        let url = &url;
        async move {
            let value: serde_json::Value = client
                .post(url.clone())
                .json(&json!({
                    "jsonrpc":"2.0", "id":1, "method":method, "params":params
                }))
                .send()
                .await?
                .json()
                .await?;
            eyre::ensure!(value.get("error").is_none(), "RPC {method}: {value}");
            Ok::<_, eyre::Report>(value["result"].clone())
        }
    };
    for block in ["latest", "pending", "0x0"] {
        assert_eq!(
            rpc("eth_getTransactionCount", json!([signer.address(), block, ["0x1", "0x2"]]))
                .await?,
            "0x0"
        );
        assert_eq!(
            rpc("eth_getTransactionCount", json!([signer.address(), block, ["0x0"]])).await?,
            rpc("eth_getTransactionCount", json!([signer.address(), block])).await?
        );
    }
    assert_eq!(rpc("eth_getTransactionCount", json!([signer.address()])).await?, "0x0");
    assert_eq!(
        rpc(
            "eth_getTransactionCount",
            json!([signer.address(), "latest", [format!("{:#x}", U256::MAX)]])
        )
        .await?,
        "0x0"
    );
    let keys = (1..=16).map(|key| format!("0x{key:x}")).collect::<Vec<_>>();
    assert_eq!(
        rpc("eth_getTransactionCount", json!([signer.address(), "latest", keys])).await?,
        "0x0"
    );
    for keys in [
        json!([]),
        json!(["0x1", "0x1"]),
        json!(["0x2", "0x1"]),
        json!(["0x0", "0x1"]),
        json!((1..=17).map(|key| format!("0x{key:x}")).collect::<Vec<_>>()),
        json!([format!("0x1{}", "0".repeat(64))]),
        json!(["bad"]),
    ] {
        let response: serde_json::Value = client
            .post(url.clone())
            .json(&json!({
                "jsonrpc":"2.0", "id":1, "method":"eth_getTransactionCount",
                "params":[signer.address(), "latest", keys]
            }))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
    let request = json!({"type":"0x6", "from":signer.address(), "nonceKeys":["0x1", "0x2"],
        "frames":[{"mode":"0x1", "flags":"0x3", "executionGas":"0x2710", "stateGas":"0x2fd00"}],
        "signatures":[]});
    let filled = rpc("eth_fillTransaction", json!([request.clone()])).await?;
    assert_eq!(filled["tx"]["nonceKeys"], json!(["0x1", "0x2"]));
    assert_eq!(filled["tx"]["nonce"], "0x0");
    assert!(filled["tx"].get("nonceSeq").is_none());
    let mut verify = self_verify_frame();
    verify.limits.state = 195_840;
    let mut tx = TxEip8141::decode_2718_exact(&frame_tx(
        &signer,
        0,
        vec![verify, sender_frame(recipient())],
    ))?;
    tx.nonce_keys = Some(vec![U256::from(1), U256::from(2)]);
    tx.signatures[0] = FrameSignature::from_secp256k1(
        Default::default(),
        Default::default(),
        signer.sign_hash_sync(&tx.signature_hash())?,
    )?;
    let mut raw = Vec::new();
    tx.eip2718_encode(&mut raw);
    let hash = node.rpc.inject_tx(raw.into()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let pending = rpc(
                "eth_getTransactionCount",
                json!([signer.address(), "pending", ["0x1", "0x2"]]),
            )
            .await?;
            if pending == "0x1" {
                break Ok::<_, eyre::Report>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await??;
    assert_eq!(
        rpc("eth_getTransactionCount", json!([signer.address(), "latest", ["0x1", "0x2"]])).await?,
        "0x0"
    );

    assert_eq!(rpc("eth_getTransactionCount", json!([signer.address(), "pending"])).await?, "0x0");
    assert_eq!(
        rpc("eth_getTransactionCount", json!([signer.address(), "pending", ["0x0"]])).await?,
        "0x0"
    );
    let mut disjoint = tx.clone();
    disjoint.nonce_keys = Some(vec![U256::from(3)]);
    disjoint.signatures[0] = FrameSignature::from_secp256k1(
        Default::default(),
        Default::default(),
        signer.sign_hash_sync(&disjoint.signature_hash())?,
    )?;
    let mut other_raw = Vec::new();
    disjoint.eip2718_encode(&mut other_raw);
    assert!(node.rpc.inject_tx(other_raw.into()).await.is_err());
    assert_mined_from_pool(&mut node, &[hash]).await?;
    let next = rpc("eth_fillTransaction", json!([request])).await?;
    assert_eq!(next["tx"]["nonce"], "0x1");
    let read = rpc("eth_getTransactionByHash", json!([hash])).await?;
    assert_eq!(read["nonceKeys"], json!(["0x1", "0x2"]));
    assert_eq!(read["nonce"], "0x0");
    let block = rpc("eth_getBlockByNumber", json!(["latest", true])).await?;
    assert_eq!(block["transactions"][0]["nonceKeys"], json!(["0x1", "0x2"]));
    assert_eq!(rpc("eth_getTransactionCount", json!([signer.address(), "latest"])).await?, "0x0");
    for at in [
        json!("latest"),
        json!("safe"),
        json!("finalized"),
        block["number"].clone(),
        json!({"blockHash":block["hash"], "requireCanonical":true}),
    ] {
        assert_eq!(
            rpc("eth_getTransactionCount", json!([signer.address(), at, ["0x1", "0x2"]])).await?,
            "0x1"
        );
    }
    assert_eq!(
        rpc("eth_getTransactionCount", json!([signer.address(), "0x0", ["0x1", "0x2"]])).await?,
        "0x0"
    );
    assert_eq!(
        rpc("eth_getTransactionCount", json!([recipient(), "latest", ["0x1"]])).await?,
        "0x0"
    );
    let response: serde_json::Value = client
        .post(url.clone())
        .json(&json!({
            "jsonrpc":"2.0", "id":1, "method":"eth_getTransactionCount",
            "params":[signer.address(), "latest", ["0x1", "0x3"]]
        }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(response["error"]["code"], -32000, "{response}");
    for key in [U256::from(1), U256::from(2)] {
        let stored = rpc(
            "eth_getStorageAt",
            json!([NONCE_MANAGER, nonce_slot(signer.address(), key), "latest"]),
        )
        .await?;
        assert_eq!(stored, json!(format!("0x{:064x}", 1)));
    }
    Ok(())
}

#[tokio::test]
async fn keyed_nonce_query_requires_activation() -> eyre::Result<()> {
    use serde_json::json;
    let spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .bogota_activated()
            .with_bogota_at(u64::MAX)
            .build(),
    );
    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        spec,
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let node = nodes.pop().unwrap();
    let client = reqwest::Client::new();
    for block in ["latest", "0x0", "pending"] {
        for (keys, expected_error) in
            [(json!(["0x1"]), Some(-32000)), (json!(["0x0"]), None), (json!([]), Some(-32602))]
        {
            let response: serde_json::Value = client.post(node.rpc_url()).json(&json!({
                "jsonrpc":"2.0", "id":1, "method":"eth_getTransactionCount", "params":[recipient(), block, keys]
            })).send().await?.json().await?;
            if let Some(code) = expected_error {
                assert_eq!(response["error"]["code"], code, "{response}");
            } else {
                assert_eq!(response["result"], "0x0", "{response}");
            }
        }
    }
    Ok(())
}
