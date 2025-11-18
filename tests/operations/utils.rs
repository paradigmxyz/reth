//! Utility functions for e2e functional tests

use crate::operations::{
    contracts::*, eth_block_number, eth_gas_price, eth_get_block_by_number_or_hash,
    eth_get_transaction_count, eth_get_transaction_receipt, manager::*, BlockId, HttpClient,
};
use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{hex, Address, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{sol, SolCall, SolValue};
use eyre::{eyre, Result};
use serde_json;
use std::{str::FromStr, sync::OnceLock};
use tokio::time::{sleep, Duration};

static DEPLOYED_CONTRACTS: OnceLock<DeployedContracts> = OnceLock::new();

/// Holds addresses of all deployed test contracts
#[derive(Debug, Clone)]
pub struct DeployedContracts {
    /// Address of Contract A
    pub contract_a: Address,
    /// Address of Contract B
    pub contract_b: Address,
    /// Address of Contract C
    pub contract_c: Address,
    /// Address of the contract factory
    pub factory: Address,
    /// Address of the ERC20 token contract
    pub erc20: Address,
}

/// Create a new HTTP client for the given endpoint URL
pub fn create_test_client(endpoint_url: &str) -> HttpClient {
    HttpClient::builder().build(endpoint_url).unwrap()
}

/// Transfer native balance from the rich address to a target address
pub async fn transfer_token(endpoint_url: &str, amount: U256, to_address: &str) -> Result<String> {
    let signer = PrivateKeySigner::from_str(DEFAULT_RICH_PRIVATE_KEY.trim_start_matches("0x"))?;
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(endpoint_url.parse()?);

    let from = signer.address();
    let to = Address::from_str(to_address)?;
    let nonce = provider.get_transaction_count(from).pending().await?;
    let gas_price = provider.get_gas_price().await?;
    let tx = TransactionRequest::default()
        .with_from(from)
        .with_to(to)
        .with_value(amount)
        .with_nonce(nonce)
        .with_chain_id(DEFAULT_L2_CHAIN_ID)
        .with_gas_limit(21_000)
        .with_gas_price(gas_price);
    let pending_tx = provider.send_transaction(tx).await?;

    let tx_hash = *pending_tx.tx_hash();
    println!("Tx sent: {:#x}, waiting for tx receipt confirmation.", tx_hash);

    // Wait for the transaction to be mined
    wait_for_tx_mined(endpoint_url, &format!("{:#x}", tx_hash)).await?;
    println!("Transaction {:#x} confirmed successfully", tx_hash);
    Ok(format!("{:#x}", tx_hash))
}

/// Deploys smart contract using rich address
pub async fn deploy_contract(
    endpoint_url: &str,
    bytecode_hex: &str,
    constructor_args: Option<Bytes>,
) -> Result<Address> {
    let signer = PrivateKeySigner::from_str(DEFAULT_RICH_PRIVATE_KEY.trim_start_matches("0x"))?;
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(endpoint_url.parse()?);

    let mut bytecode = hex::decode(bytecode_hex.trim_start_matches("0x"))?;
    if let Some(args) = constructor_args {
        bytecode.extend_from_slice(&args);
    }
    let data = Bytes::from(bytecode);

    let from = signer.address();
    let nonce = provider.get_transaction_count(from).pending().await?;
    let gas_price = provider.get_gas_price().await?;
    let tx = TransactionRequest::default()
        .with_from(from)
        .with_nonce(nonce)
        .with_chain_id(DEFAULT_L2_CHAIN_ID)
        .with_gas_limit(5_000_000)
        .with_gas_price(gas_price)
        .with_deploy_code(data);
    let pending = provider.send_transaction(tx).await?;

    let receipt = pending.get_receipt().await?;
    if !receipt.status() {
        return Err(eyre!("deployment failed"));
    }
    receipt
        .contract_address
        .ok_or(eyre!("Contract deployment failed, no contract address in receipt"))
}

/// Ensure all test contracts are deployed
pub async fn try_deploy_contracts() -> Result<&'static DeployedContracts> {
    // Return cached contracts if already deployed
    if let Some(contracts) = DEPLOYED_CONTRACTS.get() {
        println!("Contracts already deployed in this test run, returning cached addresses");
        return Ok(contracts);
    }

    println!("\n=== Deploying contracts ===");
    // Deploy ContractB (no constructor args)
    println!("Deploying ContractB...");
    let contract_b = deploy_contract(DEFAULT_L2_NETWORK_URL, CONTRACT_B_BYTECODE_STR, None).await?;
    println!("ContractB deployed at: {:#x}", contract_b);

    // Deploy ContractA (requires ContractB address as constructor arg)
    println!("Deploying ContractA...");
    let contract_a_constructor_args = contract_b.abi_encode();
    let contract_a = deploy_contract(
        DEFAULT_L2_NETWORK_URL,
        CONTRACT_A_BYTECODE_STR,
        Some(Bytes::from(contract_a_constructor_args)),
    )
    .await?;
    println!("ContractA deployed at: {:#x}", contract_a);

    // Deploy ContractFactory (no constructor args)
    println!("Deploying ContractFactory...");
    let factory =
        deploy_contract(DEFAULT_L2_NETWORK_URL, CONTRACT_FACTORY_BYTECODE_STR, None).await?;
    println!("ContractFactory deployed at: {:#x}", factory);

    // Deploy ContractC (no constructor args)
    println!("Deploying ContractC...");
    let contract_c = deploy_contract(DEFAULT_L2_NETWORK_URL, CONTRACT_C_BYTECODE_STR, None).await?;
    println!("ContractC deployed at: {:#x}", contract_c);

    // Deploy ERC20 (no constructor args)
    println!("Deploying ERC20...");
    let erc20 = deploy_contract(DEFAULT_L2_NETWORK_URL, ERC20_BYTECODE_STR, None).await?;
    println!("ERC20 deployed at: {:#x}", erc20);

    // Store deployed contract addresses in global state
    let contracts = DeployedContracts { contract_a, contract_b, contract_c, factory, erc20 };

    // Cache the deployed contracts (marks ContractsDeployed = true)
    DEPLOYED_CONTRACTS.set(contracts).map_err(|_| eyre!("Failed to cache deployed contracts"))?;

    println!("\n=== All contracts deployed successfully! ===");
    println!("ContractA: {:#x}", contract_a);
    println!("ContractB: {:#x}", contract_b);
    println!("ContractC: {:#x}", contract_c);
    println!("Factory: {:#x}", factory);
    println!("ERC20: {:#x}", erc20);
    println!("ContractsDeployed: true");
    Ok(DEPLOYED_CONTRACTS.get().unwrap())
}

/// Helper function to wait for blocks to be mined
pub async fn wait_for_blocks(client: &HttpClient, min_blocks: u64) -> u64 {
    let mut block_number: u64 = 0;
    for i in 0..30 {
        block_number = eth_block_number(client).await.unwrap();
        println!("Block number: {}, attempt {}", block_number, i);
        if block_number > min_blocks {
            break;
        }
        sleep(Duration::from_secs(1)).await;
    }
    block_number
}

/// Setup the test environment by waiting for blocks and returning a valid block hash and number
pub async fn setup_test_environment(client: &HttpClient) -> Result<(String, u64)> {
    // Wait for at least one block to be available
    let block_number = wait_for_blocks(client, 0).await;

    if block_number == 0 {
        return Err(eyre!("Block number should be greater than 0"));
    }

    // Get block data by number
    let block_data =
        eth_get_block_by_number_or_hash(client, BlockId::Number(block_number), true).await?;

    if block_data.is_null() {
        return Err(eyre!("Block data should not be null"));
    }

    // Extract block hash from the returned data
    let block_hash = if let Some(hash_value) = block_data.get("hash") {
        if let Some(hash_str) = hash_value.as_str() {
            if !hash_str.is_empty() {
                // println!("Extracted block hash: {}", hash_str);
                hash_str.to_string()
            } else {
                println!("Hash field is empty");
                return Err(eyre!("Block hash should not be empty"));
            }
        } else {
            println!("Hash field is not a string");
            return Err(eyre!("Block hash field is not a string"));
        }
    } else {
        println!("No hash field found in block data");
        return Err(eyre!("No hash field found in block data"));
    };

    if block_hash.is_empty() ||
        block_hash == "0x0000000000000000000000000000000000000000000000000000000000000000"
    {
        return Err(eyre!("Block hash should not be empty or zero"));
    }

    Ok((block_hash, block_number))
}

/// Transfer ERC20 tokens from the rich address to a target address
pub async fn erc20_transfer_tx(
    endpoint_url: &str,
    amount: U256,
    gas_price: Option<u128>,
    to_address: Address,
    erc20_address: Address,
    nonce: u64,
) -> Result<String> {
    // Define the ERC20 transfer function
    sol! {
        function transfer(address to, uint256 amount) external returns (bool);
    }

    let signer = PrivateKeySigner::from_str(DEFAULT_RICH_PRIVATE_KEY.trim_start_matches("0x"))?;
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(endpoint_url.parse()?);

    let from = signer.address();
    let gas_price = if let Some(gp) = gas_price { gp } else { provider.get_gas_price().await? };
    let call = transferCall { to: to_address, amount };
    let calldata = call.abi_encode();
    let tx = TransactionRequest::default()
        .with_from(from)
        .with_to(erc20_address)
        .with_value(U256::ZERO) // ERC20 transfers don't send ETH
        .with_input(calldata)
        .with_nonce(nonce)
        .with_chain_id(DEFAULT_L2_CHAIN_ID)
        .with_gas_limit(60_000) // Fixed gas limit for ERC20 transfers
        .with_gas_price(gas_price);
    let pending_tx = provider.send_transaction(tx).await?;

    let tx_hash = *pending_tx.tx_hash();
    Ok(format!("{:#x}", tx_hash))
}

/// Sends a batch of ERC20 token transfers and waits for them to be mined
pub async fn transfer_erc20_token_batch(
    endpoint_url: &str,
    erc20_address: Address,
    amount: U256,
    to_address: &str,
    batch_size: usize,
) -> Result<(Vec<String>, u64, String)> {
    let mut tx_hashes: Vec<String> = Vec::new();

    let to = Address::from_str(to_address)?;

    let client = create_test_client(endpoint_url);
    let gas_price = eth_gas_price(&client).await?.to::<u128>();
    let start_nonce =
        eth_get_transaction_count(&client, DEFAULT_RICH_ADDRESS, Some(BlockId::Pending)).await?;

    // Send transactions with incrementing nonces (1 to batch_size-1)
    println!("Starting batch transfer of {} transactions", batch_size);
    for i in 1..batch_size {
        let nonce = start_nonce + i as u64;
        let tx_hash =
            erc20_transfer_tx(endpoint_url, amount, Some(gas_price), to, erc20_address, nonce)
                .await?;
        tx_hashes.push(tx_hash);
        println!("Sent transaction {}/{}: {}", i, batch_size - 1, tx_hashes.last().unwrap());
    }

    // Send the last transaction with the starting nonce
    let tx_hash =
        erc20_transfer_tx(endpoint_url, amount, Some(gas_price), to, erc20_address, start_nonce)
            .await?;
    tx_hashes.push(tx_hash.clone());
    println!("Sent final transaction: {}", tx_hash);

    // Wait for all transactions to be mined
    println!("Waiting for all transactions to be mined...");
    for (i, tx_hash) in tx_hashes.iter().enumerate() {
        wait_for_tx_mined(endpoint_url, tx_hash).await?;
        println!("Transaction {}/{} mined", i + 1, tx_hashes.len());
    }
    println!("All {} transactions have been mined successfully", tx_hashes.len());

    // Get receipt of the last transaction
    let last_tx_hash = tx_hashes.last().unwrap();
    let receipt = wait_for_tx_mined(endpoint_url, last_tx_hash).await?;

    let block_number = receipt["blockNumber"]
        .as_str()
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| eyre!("Failed to parse block number from receipt"))?;

    let block_hash = receipt["blockHash"]
        .as_str()
        .ok_or_else(|| eyre!("Failed to get block hash from receipt"))?
        .to_string();

    Ok((tx_hashes, block_number, block_hash))
}

/// Waits for a transaction receipt with timeout
async fn wait_for_tx_mined(endpoint_url: &str, tx_hash: &str) -> Result<serde_json::Value> {
    let client = create_test_client(endpoint_url);
    tokio::time::timeout(DEFAULT_TIMEOUT_TX_TO_BE_MINED, async {
        loop {
            if let Ok(receipt) = eth_get_transaction_receipt(&client, tx_hash).await {
                if !receipt.is_null() {
                    let status = receipt["status"]
                        .as_str()
                        .ok_or(eyre!("tx receipt missing status field"))?;
                    if status == "0x1" {
                        return Ok(receipt);
                    } else {
                        return Err(eyre!("tx execution failed with status: {}", status));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| eyre!("timeout waiting for tx to be mined: {}", tx_hash))?
}

/// Send the tx request and wait for it to be mined
pub async fn sign_and_send_transaction(
    endpoint_url: &str,
    private_key: &str,
    tx_request: TransactionRequest,
) -> Result<(String, serde_json::Value)> {
    let key_str = private_key.trim_start_matches("0x");
    let signer = PrivateKeySigner::from_str(key_str)
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(endpoint_url.parse()?);

    let pending_tx = provider
        .send_transaction(tx_request)
        .await
        .map_err(|e| eyre!("Failed to send transaction: {}", e))?;

    let tx_hash = *pending_tx.tx_hash();
    println!("tx sent: {:#x}", tx_hash);

    let receipt = wait_for_tx_mined(endpoint_url, &format!("{:#x}", tx_hash)).await?;
    let block_number = receipt["blockNumber"]
        .as_str()
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| eyre!("Failed to parse block number"))?;
    let gas_used = receipt["gasUsed"]
        .as_str()
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| eyre!("Failed to parse gas used"))?;
    println!("tx mined in block {}, gas used: {}", block_number, gas_used);

    Ok((format!("{:#x}", tx_hash), receipt))
}

/// Extracts the refund counter for a specific opcode from a debug trace result
pub fn get_refund_counter_from_trace(trace_result: &serde_json::Value, opcode: &str) -> u64 {
    let mut refund_counter = 0u64;

    // Get the structLogs array
    let struct_logs = match trace_result["structLogs"].as_array() {
        Some(logs) => logs,
        None => return refund_counter,
    };

    // Iterate through each log entry
    for entry in struct_logs {
        // Check if this log has the matching opcode
        if let Some(op) = entry["op"].as_str() {
            if op == opcode {
                // Extract the refund value
                if let Some(refund) = entry["refund"].as_u64() {
                    refund_counter = refund;
                    break;
                } else if let Some(refund) = entry["refund"].as_f64() {
                    refund_counter = refund as u64;
                    break;
                }
            }
        }
    }

    refund_counter
}
