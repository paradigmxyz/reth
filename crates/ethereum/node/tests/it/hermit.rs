//! A native node workload for running the test executable under Hermit's scheduler.

use alloy_network::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionRequest};
use jsonrpsee_core::client::ClientT;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_db::test_utils::create_test_rw_db;
use reth_e2e_test_utils::{transaction::TransactionTestContext, wallet::Wallet};
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_node_core::{
    args::{DatadirArgs, RpcServerArgs},
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_provider::{BlockNumReader, DatabaseProviderFactory};
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use reth_tasks::Runtime;
use reth_transaction_pool::TransactionPool;
use serde_json::{json, Value};
use std::{str::FromStr, sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "native node guest for the Hermit determinism demonstration"]
async fn hermit_native_node_transfers() -> eyre::Result<()> {
    let runtime = Runtime::test();
    let directory = tempfile::tempdir()?;
    let chain = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .cancun_activated()
            .build(),
    );
    let mut rpc = RpcServerArgs::default().with_http();
    rpc.http_api =
        Some(RpcModuleSelection::from_iter([RethRpcModule::Eth, RethRpcModule::Testing]));
    rpc.ipcdisable = true;
    let mut config = NodeConfig::test()
        .with_chain(chain.clone())
        .with_datadir_args(DatadirArgs {
            datadir: MaybePlatformPath::<DataDirPath>::from_str(
                directory.path().to_str().unwrap(),
            )?,
            static_files_path: Some(directory.path().join("static")),
            rocksdb_path: Some(directory.path().join("rocksdb")),
            pprof_dumps_path: Some(directory.path().join("pprof")),
        })
        .with_rpc(rpc)
        .with_unused_ports()
        .with_disabled_discovery();
    config.network.bootnodes = Some(vec![]);
    config.network.max_outbound_peers = Some(0);
    config.network.max_inbound_peers = Some(0);
    config.network.nat = "none".parse()?;
    config.network.p2p_secret_key_hex = Some(B256::repeat_byte(1));
    config.engine.persistence_threshold = 1;
    config.engine.memory_block_buffer_target = Some(0);
    config.engine.num_state_masking_blocks = 0;
    let builder = NodeBuilder::new(config)
        .with_database(create_test_rw_db())
        .with_launch_context(runtime.clone())
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default());
    let launcher = builder.engine_api_launcher();
    let node = builder.launch_with(launcher).await?;
    let client = node.node.rpc_server_handle().http_client().expect("HTTP enabled");
    let wallet = Wallet::default();
    let sender = wallet.inner.address();
    let recipient = Address::repeat_byte(0x42);
    let initial_balance: U256 =
        client.request("eth_getBalance", (sender, BlockNumberOrTag::Latest)).await?;
    let mut parent = chain.genesis_hash();
    let mut total_fees = U256::ZERO;
    let mut transcript = Vec::new();

    // Fixed inputs keep the chain invariant while Hermit varies native thread scheduling.
    // Five transactions also reach the engine's speculative prewarming threshold.
    for height in 1..=3u64 {
        let mut hashes = Vec::new();
        for nonce in (height - 1) * 5..height * 5 {
            let transaction = TransactionTestContext::sign_tx(
                wallet.inner.clone(),
                TransactionRequest {
                    chain_id: Some(1),
                    nonce: Some(nonce),
                    gas: Some(21_000),
                    max_fee_per_gas: Some(1_000_000_000_000),
                    max_priority_fee_per_gas: Some(20_000_000_000),
                    to: Some(recipient.into()),
                    value: Some(U256::from(100)),
                    ..Default::default()
                },
            )
            .await;
            let encoded = Bytes::from(transaction.encoded_2718());
            let hash: B256 = client.request("eth_sendRawTransaction", (encoded,)).await?;
            hashes.push(hash);
        }
        let attributes = EthPayloadAttributes {
            timestamp: height * 12,
            prev_randao: B256::repeat_byte(2),
            suggested_fee_recipient: Address::repeat_byte(3),
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
            ..Default::default()
        };
        let hash: B256 = client
            .request(
                "testing_commitBlockV1",
                (attributes, Option::<Vec<Bytes>>::None, Some(Bytes::from_static(b"hermit"))),
            )
            .await?;
        let block: Value =
            client.request("eth_getBlockByNumber", (BlockNumberOrTag::Latest, false)).await?;
        assert_eq!(block["hash"], json!(hash));
        assert_eq!(block["parentHash"], json!(parent));
        assert_eq!(block["number"], json!(format!("{height:#x}")));
        assert_eq!(block["transactions"], json!(hashes));
        assert_eq!(block["gasUsed"], json!(format!("{:#x}", 5 * 21_000)));
        for (index, tx_hash) in hashes.iter().enumerate() {
            let receipt: Value = client.request("eth_getTransactionReceipt", (tx_hash,)).await?;
            assert_eq!(receipt["blockHash"], json!(hash));
            assert_eq!(receipt["transactionIndex"], json!(format!("{index:#x}")));
            assert_eq!(receipt["status"], json!("0x1"));
            let gas: U256 = serde_json::from_value(receipt["gasUsed"].clone())?;
            assert_eq!(gas, U256::from(21_000));
            let price: U256 = serde_json::from_value(receipt["effectiveGasPrice"].clone())?;
            total_fees += gas * price;
        }
        transcript.push(json!({
            "number": height,
            "hash": hash,
            "stateRoot": block["stateRoot"],
            "receiptsRoot": block["receiptsRoot"],
            "transactions": hashes,
        }));
        parent = hash;
        // The next batch depends on the production pool maintenance actor observing this head.
        tokio::time::timeout(Duration::from_secs(30), async {
            while node.node.pool.block_info().last_seen_block_hash != hash {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
    }

    let nonce: U256 =
        client.request("eth_getTransactionCount", (sender, BlockNumberOrTag::Latest)).await?;
    let sender_balance: U256 =
        client.request("eth_getBalance", (sender, BlockNumberOrTag::Latest)).await?;
    let recipient_balance: U256 =
        client.request("eth_getBalance", (recipient, BlockNumberOrTag::Latest)).await?;
    assert_eq!(nonce, U256::from(15));
    assert_eq!(recipient_balance, U256::from(1_500));
    assert_eq!(sender_balance, initial_balance - total_fees - recipient_balance);

    // Observe a real background commit, including the memory-to-disk provider boundary.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if node.node.provider.database_provider_ro()?.best_block_number()? >= 2 {
                return eyre::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    for (index, expected) in transcript.iter().enumerate() {
        let block: Value =
            client.request("eth_getBlockByNumber", (format!("{:#x}", index + 1), false)).await?;
        assert_eq!(block["hash"], expected["hash"]);
        assert_eq!(block["stateRoot"], expected["stateRoot"]);
    }
    println!(
        "RETH_HERMIT_RESULT {}",
        json!({
            "blocks": transcript,
            "senderNonce": nonce,
            "senderBalance": sender_balance,
            "recipientBalance": recipient_balance,
        })
    );
    let shutdown_runtime = runtime.clone();
    let stopped = tokio::task::spawn_blocking(move || {
        shutdown_runtime.graceful_shutdown_with_timeout(Duration::from_secs(30))
    })
    .await?;
    assert!(stopped, "native node did not drain its graceful tasks");
    Ok(())
}
