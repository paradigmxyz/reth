//! Integration tests for e2e operations
//!
//! Run all tests with: `cargo test -p xlayer-e2e-test -- --nocapture --test-threads=1`
//! or run a specific test with: `cargo test -p xlayer-e2e-test <test_name> -- --nocapture --test-threads=1`

use xlayer_e2e_test::operations;
use alloy_primitives::U256;


#[tokio::test]
async fn test_send_transaction() {
    use alloy_primitives::U256;
    
    // Transfer 1 ETH to test account 1
    let amount = U256::from(1_000_000_000_000_000_000u128); // 1 ETH in wei
    let to_address = operations::manager::DEFAULT_L2_NEW_ACC1_ADDRESS;
    
    let tx_hash = operations::transfer_token(
        operations::manager::DEFAULT_L2_NETWORK_URL,
        amount,
        to_address,
    )
    .await
    .expect("Failed to transfer tokens");
    
    println!("Transaction hash: {}", tx_hash);
    assert!(tx_hash.starts_with("0x"));
}

#[rstest::rstest]
#[case::chain_id("EthChainId")]
#[case::eth_syncing("EthSyncing")]
#[case::eth_get_balance("EthGetBalance")]
#[case::eth_get_code("EthGetCode")]
#[case::eth_get_block_number("EthGetBlockNumber")]
#[case::eth_get_transaction_count("EthGetTransactionCount")]
#[case::eth_gas_price("EthGasPrice")]
#[case::eth_get_storage_at("EthGetStorageAt")]
#[tokio::test]
async fn test_ethereum_basic_rpc(#[case] test_name: &str) {
    let client = operations::create_test_client();
    let test_address = operations::manager::DEFAULT_RICH_ADDRESS;
    
    match test_name {
        "EthChainId" => {
            let chain_id = operations::eth_chain_id(&client)
                .await
                .expect("Failed to get chain ID");
            
            assert_eq!(chain_id, operations::manager::DEFAULT_L2_CHAIN_ID);
            println!("EthChainId result: {}", chain_id);
        }
        "EthSyncing" => {
            let syncing = operations::eth_syncing(&client)
                .await
                .expect("Failed to get syncing status");
            
            println!("EthSyncing result: {:?}", syncing);
            // The result can be either false (not syncing) or an object with sync info
        }
        "EthGetBalance" => {
            let balance = operations::get_balance(&client, test_address)
                .await
                .expect("Failed to get balance");
            
            println!("EthGetBalance result for test address: {}", balance);
            assert!(balance > U256::ZERO, "Balance should be greater than 0");
        }
        "EthGetCode" => {
            let code = operations::eth_get_code(&client, test_address, "latest")
                .await
                .expect("Failed to get code");
            
            println!("EthGetCode result length: {}", code.len());
            // Code should be a valid hex string (at least "0x")
            assert!(code.starts_with("0x"), "Code should start with 0x");
        }
        "EthGetBlockNumber" => {
            let block_number = operations::eth_block_number(&client)
                .await
                .expect("Failed to get block number");
            
            assert!(block_number > 0, "Block number should be greater than 0");
            println!("EthBlockNumber result: {}", block_number);
        }
        "EthGetTransactionCount" => {
            let tx_count = operations::eth_get_transaction_count(&client, test_address, "latest")
                .await
                .expect("Failed to get transaction count");
            
            println!("EthGetTransactionCount result: {}", tx_count);
            // Transaction count should be >= 0 (it can be 0 for new addresses)
        }
        "EthGasPrice" => {
            let gas_price = operations::eth_gas_price(&client)
                .await
                .expect("Failed to get gas price");
            
            assert!(gas_price > U256::ZERO, "Gas price should be greater than 0");
            println!("EthGasPrice result: {}", gas_price);
        }
        "EthGetStorageAt" => {
            let storage = operations::eth_get_storage_at(&client, test_address, "0x0", "latest")
                .await
                .expect("Failed to get storage");
            
            println!("EthGetStorageAt result: {}", storage);
            // Storage should be a valid hex string
            assert!(storage.starts_with("0x"), "Storage should start with 0x");
        }
        _ => panic!("Unknown test case: {}", test_name),
    }
}

#[rstest::rstest]
#[case::trace_by_hash("DebugTraceBlockByHash")]
#[case::trace_by_number("DebugTraceBlockByNumber")]
#[case::trace_transaction("DebugTraceTransaction")]
#[tokio::test]
async fn test_debug_trace_rpc(#[case] test_name: &str) {
    let client = operations::create_test_client();
    
    // Wait for blocks to be mined
    let block_number = operations::wait_for_blocks(&client, 3).await;
    assert!(block_number > 0, "Block number should be greater than 0");
    
    match test_name {
        "DebugTraceBlockByHash" => {
            // Get block by number to extract hash
            let block = operations::eth_get_block_by_number(&client, block_number, false)
                .await
                .expect("Failed to get block");
            
            let block_hash = block["hash"]
                .as_str()
                .expect("Block hash should not be empty");
            
            assert_ne!(block_hash, "", "Block hash should not be empty");
            
            // Test debug_traceBlockByHash
            let trace_result = operations::debug_trace_block_by_hash(&client, block_hash)
                .await
                .expect("Failed to trace block by hash");
            
            assert!(!trace_result.is_null(), "Trace result should not be null");
        }
        "DebugTraceBlockByNumber" => {
            // Test debug_traceBlockByNumber
            let trace_result = operations::debug_trace_block_by_number(&client, block_number)
                .await
                .expect("Failed to trace block by number");
            
            assert!(!trace_result.is_null(), "Trace result should not be null");
        }
        "DebugTraceTransaction" => {
            // Get block by number to extract hash
            let block = operations::eth_get_block_by_number(&client, block_number, false)
                .await
                .expect("Failed to get block");
            
            let block_hash = block["hash"]
                .as_str()
                .expect("Block hash should not be empty");
            
            // Get block by hash with full transaction details
            let block_info = operations::eth_get_block_by_hash(&client, block_hash, true)
                .await
                .expect("Failed to get block by hash");
            
            // Extract transactions array
            let transactions = block_info["transactions"]
                .as_array()
                .expect("Transactions field not found in block data");
            
            assert!(!transactions.is_empty(), "No transactions found in block");
            
            // Get the first transaction's hash
            let tx_hash = transactions[0]["hash"]
                .as_str()
                .expect("Transaction hash not found in transaction data");
            
            assert_ne!(tx_hash, "", "Transaction hash should not be empty");
            
            // Test debug_traceTransaction
            let trace_result = operations::debug_trace_transaction(&client, tx_hash)
                .await
                .expect("Failed to trace transaction");
            
            assert!(!trace_result.is_null(), "Trace result should not be null");
        }
        _ => panic!("Unknown test case: {}", test_name),
    }
}

#[rstest::rstest]
#[case::block_by_hash("EthGetBlockByHash")]
#[case::block_by_number("EthGetBlockByNumber")]
#[case::block_transaction_count_by_hash("EthGetBlockTransactionCountByHash")]
#[case::block_transaction_count_by_number("EthGetBlockTransactionCountByNumber")]
#[case::transaction_by_block_hash_and_index("EthGetTransactionByBlockHashAndIndex")]
#[case::transaction_by_block_number_and_index("EthGetTransactionByBlockNumberAndIndex")]
#[case::block_receipts("EthGetBlockReceipts")]
#[tokio::test]
async fn test_eth_block_rpc(#[case] test_name: &str) {
    let client = operations::create_test_client();
    
    // Setup test environment
    let (block_hash, block_number) = operations::setup_test_environment(&client)
        .await
        .expect("Failed to setup test environment");
    
    println!("Using block #{} with hash: {}", block_number, block_hash);
    
    match test_name {
        "EthGetBlockByHash" => {
            let block = operations::eth_get_block_by_hash(&client, &block_hash, true)
                .await
                .expect("Failed to get block by hash");
            
            assert!(!block.is_null(), "Block should not be null");
        }
        "EthGetBlockByNumber" => {
            let block = operations::eth_get_block_by_number(&client, block_number, true)
                .await
                .expect("Failed to get block by number");
            
            assert!(!block.is_null(), "Block should not be null");
        }
        "EthGetBlockTransactionCountByHash" => {
            let tx_count = operations::eth_get_block_transaction_count_by_hash(&client, &block_hash)
                .await
                .expect("Failed to get block transaction count by hash");
            
            println!("EthGetBlockTransactionCountByHash result: {}", tx_count);
        }
        "EthGetBlockTransactionCountByNumber" => {
            let tx_count = operations::eth_get_block_transaction_count_by_number(&client, block_number)
                .await
                .expect("Failed to get block transaction count by number");
            
            println!("EthGetBlockTransactionCountByNumber result: {}", tx_count);
        }
        "EthGetTransactionByBlockHashAndIndex" => {
            let tx = operations::eth_get_transaction_by_block_hash_and_index(&client, &block_hash, "0x0")
                .await
                .expect("Failed to get transaction by block hash and index");
            
            println!("EthGetTransactionByBlockHashAndIndex result: {:?}", tx);
        }
        "EthGetTransactionByBlockNumberAndIndex" => {
            let tx = operations::eth_get_transaction_by_block_number_and_index(&client, block_number, "0x0")
                .await
                .expect("Failed to get transaction by block number and index");
            
            println!("EthGetTransactionByBlockNumberAndIndex result: {:?}", tx);
        }
        "EthGetBlockReceipts" => {
            let client = operations::create_test_client();

            // Deploy contracts and get ERC20 address
            let contracts = operations::ensure_contracts_deployed()
                .await
                .expect("Failed to deploy contracts");

            println!("ERC20 contract at: {:#x}", contracts.erc20);
            println!("Deployment address (token holder): {:#x}", contracts.deployment_address);

            let batch_size = 10; 
            let amount = U256::from(100u128) * U256::from(10u128).pow(U256::from(18u128)); // 100 tokens per transfer
            let to_address = operations::manager::DEFAULT_L2_NEW_ACC1_ADDRESS;

            println!("Performing batch transfer of {} transactions", batch_size);
            println!("Amount per transfer: {} tokens", amount / U256::from(10u128).pow(U256::from(18u128)));
            println!("Recipient: {}", to_address);

            // Perform batch ERC20 token transfers
            let (tx_hashes, target_block_number, target_block_hash) = 
                operations::transfer_erc20_token_batch(
                    operations::manager::DEFAULT_L2_NETWORK_URL,
                    contracts.erc20,
                    amount,
                    to_address,
                    batch_size,
                )
                .await
                .expect("Failed to perform batch ERC20 transfers");

            println!("Batch transfers completed in block #{} ({})", target_block_number, target_block_hash);
            println!("Number of transactions: {}", tx_hashes.len());

            // Test getting block receipts by block number
            let receipts_by_number = operations::eth_get_block_receipts(
                &client, 
                operations::BlockId::Number(target_block_number)
            )
            .await
            .expect("Failed to get block receipts by number");

            assert!(!receipts_by_number.is_null(), "Block receipts should not be null");

            if let Some(receipts_array) = receipts_by_number.as_array() {
                println!("Number of receipts in block: {}", receipts_array.len());
                assert!(receipts_array.len() >= batch_size, 
                    "Should have at least {} receipts in the block", batch_size);
            } else {
                panic!("Block receipts should be an array");
            }

            // Test getting block receipts by block hash
            let receipts_by_hash = operations::eth_get_block_receipts(
                &client, 
                operations::BlockId::Hash(target_block_hash.clone())
            )
            .await
            .expect("Failed to get block receipts by hash");

            assert!(!receipts_by_hash.is_null(), "Block receipts by hash should not be null");

            if let Some(receipts_array) = receipts_by_hash.as_array() {
                println!("Number of receipts in block (by hash): {}", receipts_array.len());
                assert!(receipts_array.len() >= batch_size, 
                    "Should have at least {} receipts in the block", batch_size);
            } else {
                panic!("Block receipts by hash should be an array");
            }

            println!("\nERC20 batch transfer test passed!");
        }
        _ => panic!("Unknown test case: {}", test_name),
    }
}

#[rstest::rstest]
#[case::estimate_gas_simple_transfer("EthEstimateGasSimpleTransfer")]
#[case::estimate_gas_contract_call("EthEstimateGasContractCall")]
#[case::eth_call("EthCall")]
#[case::get_transaction_by_hash("EthGetTransactionByHash")]
#[case::get_transaction_receipt("EthGetTransactionReceipt")]
#[tokio::test]
async fn test_eth_transaction_rpc(#[case] test_name: &str) {
    use alloy_sol_types::{sol, SolCall};
    use alloy_primitives::hex;
    
    let client = operations::create_test_client();
    
    // Setup: Ensure contracts are deployed
    let contracts = operations::ensure_contracts_deployed()
        .await
        .expect("Failed to deploy contracts");
    
    // Setup: Send a transaction to test with
    let tx_hash = operations::transfer_token(
        operations::manager::DEFAULT_L2_NETWORK_URL,
        U256::from(1_000_000_000u64), // 1 Gwei
        operations::manager::DEFAULT_L2_NEW_ACC1_ADDRESS,
    )
    .await
    .expect("Failed to send transaction");
    
    println!("Test transaction hash: {}", tx_hash);
    
    match test_name {
        "EthEstimateGasSimpleTransfer" => {
            // Check balance first
            let balance = operations::get_balance(&client, operations::manager::DEFAULT_RICH_ADDRESS)
                .await
                .expect("Failed to get balance");
            assert!(balance > U256::ZERO, "From address should have balance");
            
            let transfer_amount = U256::from(1_000_000_000_000_000u128); // 0.001 ETH
            let gas = operations::estimate_gas(
                &client,
                Some(serde_json::json!({
                    "from": operations::manager::DEFAULT_RICH_ADDRESS,
                    "to": operations::manager::DEFAULT_L2_NEW_ACC1_ADDRESS,
                    "value": format!("{:#x}", transfer_amount),
                }))
            )
            .await
            .expect("Failed to estimate gas for simple transfer");
            
            assert_eq!(gas, 21000, "Simple transfer should use exactly 21000 gas");
            println!("EthEstimateGas SimpleTransfer result: {} gas", gas);
        }
        "EthEstimateGasContractCall" => {
            sol! {
                function triggerCall() external;
            }
            let call = triggerCallCall {};
            let calldata = call.abi_encode();
            
            let gas = operations::estimate_gas(
                &client,
                Some(serde_json::json!({
                    "from": operations::manager::DEFAULT_RICH_ADDRESS,
                    "to": format!("{:#x}", contracts.contract_a),
                    "data": format!("0x{}", hex::encode(&calldata)),
                }))
            )
            .await
            .expect("Failed to estimate gas for contract call");
            
            assert!(gas > 21000, "Contract call should use more than 21000 gas");
            println!("EthEstimateGas ContractCall result: {} gas", gas);
        }
        "EthCall" => {
            let balance_before = operations::get_balance(&client, operations::manager::DEFAULT_RICH_ADDRESS)
                .await
                .expect("Failed to get balance before call");
            assert!(balance_before > U256::ZERO, "From address should have balance");
            
            sol! {
                function getValue() external view returns (uint256);
            }
            let get_value_call = getValueCall {};
            let calldata = get_value_call.abi_encode();
            
            let result = operations::eth_call(
                &client,
                Some(serde_json::json!({
                    "from": operations::manager::DEFAULT_RICH_ADDRESS,
                    "to": format!("{:#x}", contracts.contract_c),
                    "data": format!("0x{}", hex::encode(&calldata)),
                }))
            )
            .await
            .expect("Failed to execute eth_call");
            
            assert!(result.starts_with("0x"), "Call result should start with 0x");
            println!("EthCall result: {}", result);
            
            let balance_after = operations::get_balance(&client, operations::manager::DEFAULT_RICH_ADDRESS)
                .await
                .expect("Failed to get balance after call");
            assert_eq!(balance_before, balance_after, "Balance should remain unchanged after eth_call");
            println!("Balance unchanged after eth_call ✓");
        }
        "EthGetTransactionByHash" => {
            let tx_data = operations::eth_get_transaction_by_hash(&client, &tx_hash)
                .await
                .expect("Failed to get transaction by hash");
            
            assert!(!tx_data.is_null(), "Transaction data should not be null");
            
            let tx_hash_from_data = tx_data["hash"]
                .as_str()
                .expect("Transaction should have hash field");
            assert_eq!(tx_hash, tx_hash_from_data, "Transaction hash should match");
            println!("Transaction hash matches: {}", tx_hash_from_data);
            
            let from_addr_str = tx_data["from"]
                .as_str()
                .expect("Transaction should have from field");
            assert_eq!(
                from_addr_str.to_lowercase(),
                operations::manager::DEFAULT_RICH_ADDRESS.to_lowercase(),
                "From address should match"
            );
            println!("From address matches: {}", from_addr_str);
        }
        "EthGetTransactionReceipt" => {
            let receipt = operations::eth_get_transaction_receipt(&client, &tx_hash)
                .await
                .expect("Failed to get transaction receipt");
            
            assert!(!receipt.is_null(), "Receipt should not be null");
            
            let from_receipt = receipt["from"]
                .as_str()
                .expect("Receipt should have from field");
            assert_eq!(
                from_receipt.to_lowercase(),
                operations::manager::DEFAULT_RICH_ADDRESS.to_lowercase(),
                "From address should match sender"
            );
            println!("Receipt from address: {}", from_receipt);
            
            let status = receipt["status"]
                .as_str()
                .expect("Receipt should have status field");
            assert_eq!(status, "0x1", "Status should be 0x1 for successful transaction");
            println!("Transaction status: {} (successful)", status);
            
            let to_receipt = receipt["to"]
                .as_str()
                .expect("Receipt should have to field");
            assert_eq!(
                to_receipt.to_lowercase(),
                operations::manager::DEFAULT_L2_NEW_ACC1_ADDRESS.to_lowercase(),
                "To address should match recipient"
            );
            println!("Receipt to address: {}", to_receipt);
            
            let tx_hash_from_receipt = receipt["transactionHash"]
                .as_str()
                .expect("Receipt should have transactionHash field");
            assert_eq!(tx_hash, tx_hash_from_receipt, "Transaction hash should match");
            println!("Transaction hash from receipt matches: {}", tx_hash_from_receipt);
            
            // Verify transaction index matches between transaction and receipt
            let tx_data = operations::eth_get_transaction_by_hash(&client, &tx_hash)
                .await
                .expect("Failed to get transaction by hash");
            
            let tx_index_from_tx = tx_data["transactionIndex"]
                .as_str()
                .expect("Transaction should have transactionIndex field");
            let tx_index_from_receipt = receipt["transactionIndex"]
                .as_str()
                .expect("Receipt should have transactionIndex field");
            assert_eq!(
                tx_index_from_tx, tx_index_from_receipt,
                "Transaction index should match between transaction and receipt"
            );
            println!("Transaction index matches: {}", tx_index_from_tx);
        }
        _ => panic!("Unknown test case: {}", test_name),
    }
}

#[tokio::test]
async fn test_eth_logs_rpc() {
    let client = operations::create_test_client();

    let (block_hash, block_number) = operations::setup_test_environment(&client)
        .await
        .expect("Failed to setup test environment");
    println!("Using block {} with hash: {}", block_number, block_hash);
    let from_block = format!("0x{:x}", block_number);
    let to_block = format!("0x{:x}", block_number);
    let address = "0x1234567890123456789012345678901234567890";

    let logs = operations::eth_get_logs(&client, &from_block, &to_block, &address)
        .await
        .expect("Failed to get logs");

    assert!(!logs.is_null(), "Logs should not be null");
    println!("EthGetLogs result type: {}", logs);
}

#[rstest::rstest]
#[case::txpool_content("TxPoolContent")]
#[case::txpool_status("TxPoolStatus")]
#[tokio::test]
async fn test_txpool_rpc(#[case] test_name: &str) {
    let client = operations::create_test_client();

    // Setup test environment to ensure the node is running
    let (_block_hash, _block_number) = operations::setup_test_environment(&client)
        .await
        .expect("Failed to setup test environment");

    match test_name {
        "TxPoolContent" => {
            // Test txpool_content - This might return a large object, so only log type
            let content = operations::txpool_content(&client)
                .await
                .expect("Failed to get tx pool content");

            assert!(!content.is_null(), "TxPool content should not be null");
            println!("TxPoolContent result: {}", content);

            // The result should be an object with "pending" and "queued" fields
            if let Some(obj) = content.as_object() {
                assert!(obj.contains_key("pending") || obj.contains_key("queued"), 
                    "TxPool content should have pending or queued fields");
                println!("TxPool has {} top-level keys", obj.len());
            }
        }
        "TxPoolStatus" => {
            // Test txpool_status
            let status = operations::txpool_status(&client)
                .await
                .expect("Failed to get tx pool status");

            assert!(!status.is_null(), "TxPool status should not be null");
            println!("TxPoolStatus result: {}", status);

            // The result should be an object with "pending" and "queued" counts
            if let Some(obj) = status.as_object() {
                // Verify the status has pending and queued fields
                if let Some(pending) = obj.get("pending") {
                    println!("Pending transactions: {}", pending);
                }
                if let Some(queued) = obj.get("queued") {
                    println!("Queued transactions: {}", queued);
                }
            }
        }
        _ => panic!("Unknown test case: {}", test_name),
    }
}

#[rstest::rstest]
#[case::eip1559_simple_transfer("Eip1559SimpleTransfer")]
#[case::eip1559_contract_call("Eip1559ContractCall")]
#[case::eth_fee_history("EthFeeHistory")]
#[case::eip2930_access_list("Eip2930AccessList")]
#[case::eip3198_basefee_opcode("Eip3198BasefeeOpcode")]
#[case::eip3529_reduced_refunds("Eip3529ReducedRefunds")]
#[case::eip4844_blob_fields("Eip4844BlobFields")]
#[tokio::test]
async fn test_new_transaction_types(#[case] test_name: &str) {
    use alloy_network::TransactionBuilder;
    use alloy_primitives::{Address, B256, Bytes, U256, hex};
    use alloy_rpc_types_eth::{AccessList, AccessListItem, TransactionRequest};
    use alloy_signer_local::PrivateKeySigner;
    use alloy_sol_types::{sol, SolCall};
    use jsonrpsee::core::client::ClientT;
    use serde_json::json;
    use std::str::FromStr;

    let client = operations::create_test_client();
    let private_key = "363ea277eec54278af051fb574931aec751258450a286edce9e1f64401f3b9c8";

    match test_name {
        "Eip1559SimpleTransfer" => {
            operations::ensure_contracts_deployed()
                .await
                .expect("Failed to deploy contracts");

            let signer = PrivateKeySigner::from_str(private_key).expect("Invalid private key");
            let from_address = signer.address();
            let to_address = Address::from_str("0x1111111111111111111111111111111111111111").unwrap();

            let funding_amount = U256::from(10_000_000_000_000_000_000u128); // 10 ETH
            operations::transfer_token(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                funding_amount,
                &format!("{:#x}", from_address),
            )
            .await
            .expect("Failed to fund test address");

            println!("Funded test address {:#x} with 10 ETH", from_address);

            // Create EIP-1559 transaction
            let value = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
            let tx_request = TransactionRequest::default()
                .to(to_address)
                .value(value)
                .with_gas_limit(21000)
                .with_max_fee_per_gas(20_000_000_000u128)
                .with_max_priority_fee_per_gas(1_000_000_000u128);

            let (tx_hash, receipt) = operations::sign_and_send_transaction(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                private_key,
                tx_request,
            )
            .await
            .expect("Failed to send EIP-1559 transaction");

            // Verify transaction
            let status = receipt["status"].as_str().unwrap();
            assert_eq!(status, "0x1", "Transaction should succeed");

            let gas_used = receipt["gasUsed"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap();
            assert_eq!(gas_used, 21000, "Should use exactly 21000 gas for simple transfer");

            let tx_type = receipt["type"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap();
            assert_eq!(tx_type, 2, "Receipt type should be EIP-1559 (type 2)");

            println!("EIP-1559 transfer successful: {}, gas used: {}", tx_hash, gas_used);
        }
        "Eip1559ContractCall" => {
            // Test for EIP-1559 contract call
            let contracts = operations::ensure_contracts_deployed()
                .await
                .expect("Failed to deploy contracts");

            let signer = PrivateKeySigner::from_str(private_key).expect("Invalid private key");
            let from_address = signer.address();

            let funding_amount = U256::from(10_000_000_000_000_000_000u128);
            operations::transfer_token(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                funding_amount,
                &format!("{:#x}", from_address),
            )
            .await
            .expect("Failed to fund test address");

            // Prepare contract call
            sol! {
                function triggerCall() external;
            }
            let call = triggerCallCall {};
            let calldata = call.abi_encode();

            let tx_request = TransactionRequest::default()
                .to(contracts.contract_a)
                .with_gas_limit(200_000)
                .with_max_fee_per_gas(20_000_000_000u128)
                .with_max_priority_fee_per_gas(2_000_000_000u128)
                .with_input(calldata);

            let (tx_hash, receipt) = operations::sign_and_send_transaction(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                private_key,
                tx_request,
            )
            .await
            .expect("Failed to send contract call");

            let status = receipt["status"].as_str().unwrap();
            assert_eq!(status, "0x1", "Contract call should succeed");

            let gas_used = receipt["gasUsed"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap();
            assert!(gas_used > 21000, "Contract call should use more than 21000 gas");

            println!("EIP-1559 contract call successful: {}, gas used: {}", tx_hash, gas_used);
        }
        "EthFeeHistory" => {
            // Test for eth_feeHistory RPC
            let contracts = operations::ensure_contracts_deployed()
                .await
                .expect("Failed to deploy contracts");

            // Perform batch transfers to generate blocks
            let batch_size = 10;
            let amount = U256::from(1_000_000_000u128); // 1 Gwei
            let to_address = "0x1111111111111111111111111111111111111111";

            operations::transfer_erc20_token_batch(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                contracts.erc20,
                amount,
                to_address,
                batch_size,
            )
            .await
            .expect("Failed to perform batch transfers");

            // Call eth_feeHistory for last 20 blocks
            let block_count = 20; // 20 blocks
            let result: serde_json::Value = client
                .request(
                    "eth_feeHistory",
                    jsonrpsee::rpc_params![block_count, "latest", json!(null)],
                )
                .await
                .expect("Failed to call eth_feeHistory");

            assert!(!result.is_null(), "Fee history should not be null");
            
            let oldest_block = result["oldestBlock"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .expect("Should have oldestBlock");

            let base_fees = result["baseFeePerGas"]
                .as_array()
                .expect("Should have baseFeePerGas array");

            println!("Fee history oldest block: {}, blocks returned: {}, requested: {}", 
                     oldest_block, base_fees.len(), block_count);

            for i in 0..block_count {
                let block_num = oldest_block + i;
                let block = operations::eth_get_block_by_number(&client, block_num, false)
                    .await
                    .expect("Failed to get block");

                let history_base_fee = base_fees[i as usize].as_str().expect("Base fee should be string");
                let block_base_fee = block["baseFeePerGas"].as_str().expect("Block should have baseFeePerGas");
                
                assert_eq!(
                    history_base_fee, block_base_fee,
                    "Base fee should match between fee history and block header for block {}",
                    block_num
                );
            }

            println!("eth_feeHistory test passed");
        }
        "Eip2930AccessList" => {
            // Test for EIP-2930 transactions (Type 1: Access list transactions)
            let signer = PrivateKeySigner::from_str(private_key).expect("Invalid private key");
            let from_address = signer.address();
            let to_address = Address::from_str("0x1111111111111111111111111111111111111111").unwrap();

            let funding_amount = U256::from(10_000_000_000_000_000_000u128);
            operations::transfer_token(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                funding_amount,
                &format!("{:#x}", from_address),
            )
            .await
            .expect("Failed to fund test address");

            // Get gas price
            let gas_price = operations::eth_gas_price(&client)
                .await
                .expect("Failed to get gas price");

            // Create access list
            let access_list = AccessList(vec![AccessListItem {
                address: to_address,
                storage_keys: vec![B256::ZERO],
            }]);

            let tx_request = TransactionRequest::default()
                .to(to_address)
                .with_gas_limit(26_000)
                .with_gas_price(gas_price.to::<u128>())
                .with_access_list(access_list);

            let (tx_hash, receipt) = operations::sign_and_send_transaction(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                private_key,
                tx_request,
            )
            .await
            .expect("Failed to send EIP-2930 transaction");

            let status = receipt["status"].as_str().unwrap();
            assert_eq!(status, "0x1", "EIP-2930 transaction should succeed");

            let tx_type = receipt["type"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap();
            assert_eq!(tx_type, 1, "Receipt type should be AccessListTx (type 1)");

            println!("EIP-2930 AccessListTx successful: {}", tx_hash);
        }
        "Eip3198BasefeeOpcode" => {
            // Test for EIP-3198 (BASEFEE opcode)
            let result: String = client
                .request(
                    "eth_call",
                    jsonrpsee::rpc_params![
                        json!({
                            "data": "0x4800" // BASEFEE + STOP
                        }),
                        "latest"
                    ],
                )
                .await
                .expect("BASEFEE opcode should execute without error (EIP-3198 is supported)");

            println!("BASEFEE opcode result: {}", result);

            // Verify latest block has base fee
            let latest_block = operations::eth_get_block_by_number(&client, 
                operations::eth_block_number(&client).await.unwrap(), 
                false
            )
            .await
            .expect("Failed to get latest block");

            let base_fee = latest_block["baseFeePerGas"]
                .as_str()
                .expect("Block should have baseFeePerGas field (EIP-3198)");

            let base_fee_value = u64::from_str_radix(base_fee.trim_start_matches("0x"), 16)
                .expect("Should parse base fee");

            println!("Current block base fee: {} wei", base_fee_value);
        }
        "Eip3529ReducedRefunds" => {
            // Test for EIP-3529 (Reduced refunds)
            let signer = PrivateKeySigner::from_str(private_key).expect("Invalid private key");
            let from_address = signer.address();

            let funding_amount = U256::from(10_000_000_000_000_000_000u128);
            operations::transfer_token(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                funding_amount,
                &format!("{:#x}", from_address),
            )
            .await
            .expect("Failed to fund test address");

            // Deploy contract with SELFDESTRUCT: CALLER (0x33) + SELFDESTRUCT (0xff)
            let deploy_bytecode = Bytes::from(hex::decode("33ff").unwrap());

            let tx_request = TransactionRequest::default()
                .with_deploy_code(deploy_bytecode)
                .with_gas_limit(100_000);

            let (tx_hash, receipt) = operations::sign_and_send_transaction(
                operations::manager::DEFAULT_L2_NETWORK_URL,
                private_key,
                tx_request,
            )
            .await
            .expect("Failed to deploy self-destructing contract");

            let contract_address = receipt["contractAddress"]
                .as_str()
                .expect("Should have contract address");

            println!("Contract address (self-destructed): {}, tx hash: {}", contract_address, tx_hash);

            // Trace the transaction to get refund counter
            let trace_result = operations::debug_trace_transaction(&client, &tx_hash)
                .await
                .expect("Failed to trace transaction");

            let refund_counter = operations::get_refund_counter_from_trace(&trace_result, "SELFDESTRUCT");
            println!("Refund counter after SELFDESTRUCT: {}", refund_counter);

            assert_eq!(refund_counter, 0, "SELFDESTRUCT refund should be 0 with EIP-3529");

            // Verify contract has no code after self-destruct
            let block_number = receipt["blockNumber"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap();
            
            let code = operations::eth_get_code(
                &client,
                contract_address,
                &format!("0x{:x}", block_number),
            )
            .await
            .expect("Failed to get contract code");

            assert_eq!(code, "0x", "Contract should have no code after SELFDESTRUCT");
            println!("EIP-3529 test passed: SELFDESTRUCT refund is 0");
        }
        "Eip4844BlobFields" => {
            let block = operations::eth_get_block_by_number(
                &client,
                operations::eth_block_number(&client).await.unwrap(),
                true,
            )
            .await
            .expect("Failed to get latest block");

            // OP Stack disables EIP-4844 blob transactions on Layer 2
            // So we check if blob fields are present in block data
            let blob_gas_used = block["blobGasUsed"]
                .as_str()
                .expect("Block should have blobGasUsed field");

            let excess_blob_gas = block["excessBlobGas"]
                .as_str()
                .expect("Block should have excessBlobGas field");

            println!("Block blobGasUsed: {}", blob_gas_used);
            println!("Block excessBlobGas: {}", excess_blob_gas);

            println!("EIP-4844 fields present in block data");
        }
        _ => panic!("Unknown test case: {}", test_name),
    }
}