use alloy_eips::BlockNumberOrTag;
use alloy_network::eip2718::Encodable2718;
use alloy_primitives::{Address, TxKind, B256, U256};
use alloy_rpc_types_engine::{
    ExecutionPayloadV3, ForkchoiceState, PayloadAttributes, PayloadStatusEnum,
};
use alloy_rpc_types_eth::TransactionRequest;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_e2e_test_utils::{setup, transaction::TransactionTestContext, wallet::Wallet};
use reth_optimism_chainspec::{OpChainSpecBuilder, OP_SEPOLIA};
use reth_optimism_node::OpNode;
use reth_optimism_payload_builder::OpPayloadBuilderAttributes;
use reth_optimism_primitives::OpTransactionSigned;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_provider::BlockReaderIdExt;
use reth_rpc_api::EngineApiClient;
use std::sync::Arc;

#[tokio::test]
async fn full_engine_api_bock_building_get_validation() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    tracing::debug!(target: "test", "Tracing initialized - this should appear");

    // Fixed values for deterministic state roots
    let fixed_recipient = Address::from([0x22; 20]);
    let fixed_prev_randao = B256::from([0x42; 32]);
    let fixed_fee_recipient = Address::from([0x11; 20]);

    let chain_spec = OpChainSpecBuilder::default()
        .chain(OP_SEPOLIA.chain)
        .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
        .regolith_activated()
        .canyon_activated()
        .ecotone_activated()
        .build();

    let (mut nodes, _tasks, _wallet) =
        setup::<OpNode>(1, Arc::new(chain_spec.clone()), false, |timestamp| {
            let attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            // Construct Optimism-specific payload attributes
            OpPayloadBuilderAttributes::<OpTransactionSigned> {
                payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
                transactions: vec![], // Empty vector of transactions for the builder
                no_tx_pool: false,
                gas_limit: Some(30_000_000),
                eip_1559_params: None,
                min_base_fee: None,
            }
        })
        .await?;

    let node = nodes.pop().unwrap();
    let provider = node.inner.provider.clone();

    let genesis_hash = node.block_hash(0);

    let wallet = Wallet::default();
    // Create transaction with fixed recipient for deterministic state root
    let tx = TransactionRequest {
        nonce: Some(0),
        value: Some(U256::from(100)),
        to: Some(TxKind::Call(fixed_recipient)),
        gas: Some(21000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        chain_id: Some(OP_SEPOLIA.chain.id()),
        ..Default::default()
    };
    let signed_tx = TransactionTestContext::sign_tx(wallet.inner, tx).await;
    let raw_tx = signed_tx.encoded_2718().into();
    let _tx_hash = node.rpc.inject_tx(raw_tx).await?;

    let current_head = provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
    let current_timestamp = current_head.timestamp;

    let payload_attrs = PayloadAttributes {
        timestamp: current_timestamp + 2, // 2 seconds after current block (OP block time)
        prev_randao: fixed_prev_randao,
        suggested_fee_recipient: fixed_fee_recipient,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    let fcu_state = ForkchoiceState {
        head_block_hash: genesis_hash,
        safe_block_hash: genesis_hash,
        finalized_block_hash: genesis_hash,
    };

    let op_attrs = OpPayloadAttributes {
        payload_attributes: payload_attrs.clone(),
        transactions: None,
        no_tx_pool: None,
        gas_limit: Some(30_000_000),
        eip_1559_params: None,
        min_base_fee: None,
    };

    let engine_client = node.inner.engine_http_client();
    let fcu_result = engine_client.fork_choice_updated_v3(fcu_state, Some(op_attrs)).await?;
    let payload_id = fcu_result.payload_id.expect("payload id");

    // Wait a bit for payload to be built
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Get payload from builder first (before get_payload_v3 terminates the job)
    let payload_builder_handle = node.inner.payload_builder_handle.clone();
    let built_payload = payload_builder_handle
        .best_payload(payload_id)
        .await
        .transpose()
        .ok()
        .flatten()
        .expect("Payload should be built");
    let block = Arc::new(built_payload.block().clone());

    // Now get payload via RPC to verify
    let payload_v3 = engine_client.get_payload_v3(payload_id).await?;
    assert_eq!(genesis_hash, payload_v3.execution_payload.payload_inner.payload_inner.parent_hash);
    assert_eq!(21000, payload_v3.execution_payload.payload_inner.payload_inner.gas_used);

    // newPaylaod
    let payload_v3 = ExecutionPayloadV3::from_block_unchecked(
        block.hash(),
        &Arc::unwrap_or_clone(block.clone()).into_block(),
    );
    let versioned_hashes: Vec<B256> = Vec::new();
    let parent_beacon_block_root = block.parent_beacon_block_root.unwrap_or_default();

    let new_payload_result = engine_client
        .new_payload_v3(payload_v3, versioned_hashes, parent_beacon_block_root)
        .await?;
    assert_eq!(new_payload_result.status, PayloadStatusEnum::Valid);

    // Output state root in a parseable format for comparison test
    let state_root = block.state_root;
    println!("STATE_ROOT_RESULT: {:?}", state_root);

    tracing::info!(?state_root, block_number = block.number, "Block state root after execution");

    Ok(())
}

/// Helper test that compares MDBX and TrieDB state roots by spawning two test processes.
/// This test spawns the `full_engine_api_bock_building_get_validation` test twice:
/// once with the triedb feature and once without, then compares the state roots.
#[tokio::test]
async fn test_compare_mdbx_triedb_state_roots() -> eyre::Result<()> {
    use std::process::Command;

    // Skip if we're running as a subprocess (to avoid infinite recursion)
    if std::env::var("SUBPROCESS_TEST").is_ok() {
        return Ok(());
    }

    println!("Running test to compare MDBX and TrieDB state roots...");

    // Run test without triedb feature (MDBX)
    println!("Running test with MDBX...");
    let mdbx_output = Command::new("cargo")
        .args(&[
            "test",
            "-p",
            "reth-optimism-node",
            "--test",
            "it",
            "full_engine_api_bock_building_get_validation",
            "--",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SUBPROCESS_TEST", "1")
        .output()?;

    if !mdbx_output.status.success() {
        eprintln!("MDBX test failed:");
        eprintln!("stdout: {}", String::from_utf8_lossy(&mdbx_output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&mdbx_output.stderr));
        return Err(eyre::eyre!("MDBX test failed"));
    }

    // Run test with triedb feature (TrieDB)
    println!("Running test with TrieDB...");
    let triedb_output = Command::new("cargo")
        .args(&[
            "test",
            "-p",
            "reth-optimism-node",
            "--features",
            "triedb",
            "--test",
            "it",
            "full_engine_api_bock_building_get_validation",
            "--",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SUBPROCESS_TEST", "1")
        .output()?;

    if !triedb_output.status.success() {
        eprintln!("TrieDB test failed:");
        eprintln!("stdout: {}", String::from_utf8_lossy(&triedb_output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&triedb_output.stderr));
        return Err(eyre::eyre!("TrieDB test failed"));
    }

    // Parse state roots from outputs
    let mdbx_stdout = String::from_utf8_lossy(&mdbx_output.stdout);
    let triedb_stdout = String::from_utf8_lossy(&triedb_output.stdout);

    let mdbx_state_root = extract_state_root(&mdbx_stdout)
        .ok_or_else(|| eyre::eyre!("Could not find state root in MDBX output"))?;
    let triedb_state_root = extract_state_root(&triedb_stdout)
        .ok_or_else(|| eyre::eyre!("Could not find state root in TrieDB output"))?;

    println!("MDBX state root:   {}", mdbx_state_root);
    println!("TrieDB state root: {}", triedb_state_root);

    assert_eq!(
        mdbx_state_root, triedb_state_root,
        "State roots must match between MDBX and TrieDB.\n  MDBX:   {}\n  TrieDB: {}",
        mdbx_state_root, triedb_state_root
    );

    println!("✓ State roots match between MDBX and TrieDB: {}", mdbx_state_root);

    Ok(())
}

/// Extract state root from test output
fn extract_state_root(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.contains("STATE_ROOT_RESULT:") {
            // Extract the hex string after "STATE_ROOT_RESULT: "
            if let Some(start) = line.find("0x") {
                let hex_str = &line[start..];
                // Take until whitespace or end of line
                let end = hex_str.find(char::is_whitespace).unwrap_or(hex_str.len());
                return Some(hex_str[..end].to_string());
            }
        }
    }
    None
}

#[tokio::test]
async fn full_engine_api_bock_building_continuously() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = OpChainSpecBuilder::default()
        .chain(OP_SEPOLIA.chain)
        .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
        .regolith_activated()
        .canyon_activated()
        .ecotone_activated()
        .build();

    let (mut nodes, _tasks, _wallet) =
        setup::<OpNode>(1, Arc::new(chain_spec.clone()), false, |timestamp| {
            let attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            // Construct Optimism-specific payload attributes
            OpPayloadBuilderAttributes::<OpTransactionSigned> {
                payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
                transactions: vec![], // Empty vector of transactions for the builder
                no_tx_pool: false,
                gas_limit: Some(30_000_000),
                eip_1559_params: None,
                min_base_fee: None,
            }
        })
        .await?;

    let node = nodes.pop().unwrap();
    let provider = node.inner.provider.clone();

    let genesis_hash = node.block_hash(0);

    let wallet = Wallet::default();
    let signer = wallet.inner.clone();
    let raw_tx =
        TransactionTestContext::transfer_tx_bytes(OP_SEPOLIA.chain.id(), signer.clone()).await;
    let _tx_hash = node.rpc.inject_tx(raw_tx).await?;

    let current_head = provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
    let current_timestamp = current_head.timestamp;

    let payload_attrs = PayloadAttributes {
        timestamp: current_timestamp + 2, // 2 seconds after current block (OP block time)
        prev_randao: B256::random(),
        suggested_fee_recipient: Address::random(),
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    let fcu_state = ForkchoiceState {
        head_block_hash: genesis_hash,
        safe_block_hash: genesis_hash,
        finalized_block_hash: genesis_hash,
    };

    let op_attrs = OpPayloadAttributes {
        payload_attributes: payload_attrs.clone(),
        transactions: None,
        no_tx_pool: None,
        gas_limit: Some(30_000_000),
        eip_1559_params: None,
        min_base_fee: None,
    };

    let engine_client = node.inner.engine_http_client();
    let fcu_result = engine_client.fork_choice_updated_v3(fcu_state, Some(op_attrs)).await?;
    let payload_id = fcu_result.payload_id.expect("payload id");

    // Wait a bit for payload to be built
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Get payload from builder first (before get_payload_v3 terminates the job)
    let payload_builder_handle = node.inner.payload_builder_handle.clone();
    let built_payload = payload_builder_handle
        .best_payload(payload_id)
        .await
        .transpose()
        .ok()
        .flatten()
        .expect("Payload should be built");
    let block = Arc::new(built_payload.block().clone());

    // Now get payload via RPC to verify
    let payload_v3 = engine_client.get_payload_v3(payload_id).await?;
    let block_1_hash = payload_v3.execution_payload.payload_inner.payload_inner.block_hash;
    assert_eq!(genesis_hash, payload_v3.execution_payload.payload_inner.payload_inner.parent_hash);
    assert_eq!(21000, payload_v3.execution_payload.payload_inner.payload_inner.gas_used);

    // newPaylaod
    let payload_v3 = ExecutionPayloadV3::from_block_unchecked(
        block.hash(),
        &Arc::unwrap_or_clone(block.clone()).into_block(),
    );
    let versioned_hashes: Vec<B256> = Vec::new();
    let parent_beacon_block_root = block.parent_beacon_block_root.unwrap_or_default();

    let new_payload_result = engine_client
        .new_payload_v3(payload_v3, versioned_hashes, parent_beacon_block_root)
        .await?;
    assert_eq!(new_payload_result.status, PayloadStatusEnum::Valid);
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    // Build block2
    // let raw_tx2 = TransactionTestContext::transfer_tx_bytes(chain_spec.chain.id(),
    // signer.clone()).await;
    let tx2 = TransactionRequest {
        nonce: Some(1),
        value: Some(U256::from(100)),
        to: Some(TxKind::Call(Address::random())),
        gas: Some(21000),
        max_fee_per_gas: Some(25e9 as u128), // bump fee as needed
        max_priority_fee_per_gas: Some(20e9 as u128),
        chain_id: Some(chain_spec.chain.id()),
        ..Default::default()
    };
    let signed2 = TransactionTestContext::sign_tx(signer.clone(), tx2).await;
    let raw_tx2 = signed2.encoded_2718().into();
    let _tx_hash2 = node.rpc.inject_tx(raw_tx2).await?;
    let fcu_state_2 = ForkchoiceState {
        head_block_hash: block_1_hash,
        safe_block_hash: block_1_hash,
        finalized_block_hash: genesis_hash,
    };
    let payload_attrs_2 = PayloadAttributes {
        timestamp: payload_attrs.timestamp + 2,
        prev_randao: B256::random(),
        suggested_fee_recipient: Address::random(),
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    let op_attrs_2 = OpPayloadAttributes {
        payload_attributes: payload_attrs_2.clone(),
        transactions: None,
        no_tx_pool: None,
        gas_limit: Some(30_000_000),
        eip_1559_params: None,
        min_base_fee: None,
    };
    let fcu_result_2 = engine_client.fork_choice_updated_v3(fcu_state_2, Some(op_attrs_2)).await?;
    assert_eq!(fcu_result_2.payload_status.status, PayloadStatusEnum::Valid);
    let _payload_id_2 = fcu_result_2.payload_id.expect("second payload id");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    Ok(())
}

/// Test that verifies trie states are correctly persisted to TrieDB after block production
#[tokio::test]
#[cfg(feature = "triedb")]
async fn test_triedb_state_persistence_after_block_production() -> eyre::Result<()> {
    use reth_provider::{StateProviderFactory, TrieDBProviderFactory};

    reth_tracing::init_test_tracing();
    tracing::debug!(target: "test", "Starting TrieDB state persistence test");

    // Fixed values for deterministic testing
    let fixed_recipient = Address::from([0x22; 20]);
    let transfer_amount = U256::from(100);
    let fixed_prev_randao = B256::from([0x42; 32]);
    let fixed_fee_recipient = Address::from([0x11; 20]);

    let chain_spec = OpChainSpecBuilder::default()
        .chain(OP_SEPOLIA.chain)
        .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
        .regolith_activated()
        .canyon_activated()
        .ecotone_activated()
        .build();

    // Configure TreeConfig with persistence_threshold=0 to trigger immediate persistence
    // Threshold of 0 means persist immediately without keeping blocks in memory
    let tree_config = reth_node_api::TreeConfig::default().with_persistence_threshold(0);

    let (mut nodes, _tasks, _wallet) = reth_e2e_test_utils::setup_engine::<OpNode>(
        1,
        Arc::new(chain_spec.clone()),
        false,
        tree_config,
        |timestamp| {
            let attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            OpPayloadBuilderAttributes::<OpTransactionSigned> {
                payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
                transactions: vec![],
                no_tx_pool: false,
                gas_limit: Some(30_000_000),
                eip_1559_params: None,
                min_base_fee: None,
            }
        },
    )
    .await?;

    let node = nodes.pop().unwrap();
    let provider = node.inner.provider.clone();

    let genesis_hash = node.block_hash(0);

    let wallet = Wallet::default();
    let sender_address = wallet.inner.address();

    // First, verify genesis state is in TrieDB
    let triedb_provider = provider.triedb_provider();

    // Get sender's initial balance from genesis
    let genesis_provider = provider.latest()?;
    let sender_initial_balance =
        genesis_provider.account_balance(&sender_address)?.unwrap_or_default();
    tracing::info!(
        ?sender_address,
        ?sender_initial_balance,
        "Sender initial balance from latest provider"
    );

    // Verify sender account exists in TrieDB with correct initial balance
    let sender_in_triedb = triedb_provider
        .get_account(sender_address)?
        .expect("Sender account should exist in TrieDB at genesis");

    assert_eq!(
        sender_in_triedb.balance, sender_initial_balance,
        "Sender balance in TrieDB should match genesis balance"
    );
    assert_eq!(sender_in_triedb.nonce, 0, "Sender nonce should be 0 at genesis");

    tracing::info!(
        ?sender_address,
        balance = ?sender_in_triedb.balance,
        nonce = sender_in_triedb.nonce,
        "✓ Verified sender account in TrieDB at genesis"
    );

    // Create transaction with fixed recipient
    let tx = TransactionRequest {
        nonce: Some(0),
        value: Some(transfer_amount),
        to: Some(TxKind::Call(fixed_recipient)),
        gas: Some(21000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        chain_id: Some(OP_SEPOLIA.chain.id()),
        ..Default::default()
    };
    let signed_tx = TransactionTestContext::sign_tx(wallet.inner, tx).await;
    let raw_tx = signed_tx.encoded_2718().into();
    let _tx_hash = node.rpc.inject_tx(raw_tx).await?;

    let current_head = provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
    let current_timestamp = current_head.timestamp;

    let payload_attrs = PayloadAttributes {
        timestamp: current_timestamp + 2,
        prev_randao: fixed_prev_randao,
        suggested_fee_recipient: fixed_fee_recipient,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    let fcu_state = ForkchoiceState {
        head_block_hash: genesis_hash,
        safe_block_hash: genesis_hash,
        finalized_block_hash: genesis_hash,
    };

    let op_attrs = OpPayloadAttributes {
        payload_attributes: payload_attrs.clone(),
        transactions: None,
        no_tx_pool: None,
        gas_limit: Some(30_000_000),
        eip_1559_params: None,
        min_base_fee: None,
    };

    let engine_client = node.inner.engine_http_client();
    let fcu_result = engine_client.fork_choice_updated_v3(fcu_state, Some(op_attrs)).await?;
    let payload_id = fcu_result.payload_id.expect("payload id");

    // Wait for payload to be built
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Get payload via RPC
    let payload_v3 = engine_client.get_payload_v3(payload_id).await?;
    let block_hash = payload_v3.execution_payload.payload_inner.payload_inner.block_hash;
    let state_root = payload_v3.execution_payload.payload_inner.payload_inner.state_root;

    tracing::info!(
        block_number = payload_v3.execution_payload.payload_inner.payload_inner.block_number,
        ?block_hash,
        ?state_root,
        gas_used = payload_v3.execution_payload.payload_inner.payload_inner.gas_used,
        "Block built successfully"
    );

    // Submit block via newPayload
    let versioned_hashes: Vec<B256> = Vec::new();
    let parent_beacon_block_root = B256::ZERO;

    let new_payload_result = engine_client
        .new_payload_v3(
            payload_v3.execution_payload.clone(),
            versioned_hashes,
            parent_beacon_block_root,
        )
        .await?;

    tracing::info!(?new_payload_result.status, ?new_payload_result, "newPayload result");

    // If newPayload failed, log the error and fail the test early
    if new_payload_result.status != PayloadStatusEnum::Valid {
        panic!("newPayload failed with status: {:?}", new_payload_result);
    }

    // Make block canonical via forkchoiceUpdated - this should trigger persistence since
    // threshold=1
    let fcu_state = ForkchoiceState {
        head_block_hash: block_hash,
        safe_block_hash: block_hash,
        finalized_block_hash: genesis_hash,
    };
    let fcu_result = engine_client.fork_choice_updated_v3(fcu_state, None).await?;
    tracing::info!(?fcu_result.payload_status.status, "forkchoiceUpdated result - should trigger persistence");

    // Wait longer for state to be persisted to TrieDB (persistence happens asynchronously)
    tracing::info!("Waiting 5 seconds for state persistence to TrieDB...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    tracing::info!("Wait complete, now checking TrieDB state");

    // Verify TrieDB state persistence
    let triedb_provider = provider.triedb_provider();

    // Check recipient account in TrieDB
    let recipient_account = triedb_provider
        .get_account(fixed_recipient)?
        .expect("Recipient account should exist in TrieDB");

    tracing::info!(
        ?fixed_recipient,
        balance = ?recipient_account.balance,
        nonce = recipient_account.nonce,
        "Recipient account in TrieDB"
    );

    assert_eq!(
        recipient_account.balance, transfer_amount,
        "Recipient should have received the transfer amount"
    );
    assert_eq!(recipient_account.nonce, 0, "Recipient nonce should be 0");

    // Check sender account in TrieDB
    let sender_account = triedb_provider
        .get_account(sender_address)?
        .expect("Sender account should exist in TrieDB");

    tracing::info!(
        ?sender_address,
        balance = ?sender_account.balance,
        nonce = sender_account.nonce,
        "Sender account in TrieDB"
    );

    assert_eq!(sender_account.nonce, 1, "Sender nonce should be incremented to 1");
    // Sender balance should be: initial - transfer - gas_used * gas_price
    // We just verify it's less than initial
    assert!(
        sender_account.balance < sender_initial_balance,
        "Sender balance should decrease after transfer and gas payment"
    );

    // Verify state root from TrieDB matches the block
    let triedb_state_root = triedb_provider.state_root()?;
    assert_eq!(triedb_state_root, state_root, "TrieDB state root should match block state root");

    tracing::info!(
        ?triedb_state_root,
        block_state_root = ?state_root,
        "✓ State roots match - TrieDB state persisted successfully"
    );

    Ok(())
}
