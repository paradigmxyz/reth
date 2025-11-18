//! Transaction utilities for testing

use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{hex, Address, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use eyre::{eyre, Result};
use serde_json;
use std::str::FromStr;
use tokio::time::{sleep, Duration};

use super::{
    manager::{DEFAULT_RICH_PRIVATE_KEY, DEFAULT_TIMEOUT_TX_TO_BE_MINED},
    HttpClient,
};

const TMP_SENDER_PRIVATE_KEY: &str =
    "363ea277eec54278af051fb574931aec751258450a286edce9e1f64401f3b9c8";

/// Creates an HTTP client for the local test RPC endpoint
pub fn create_test_client() -> HttpClient {
    HttpClient::builder().build("http://localhost:8124").expect("Failed to create HTTP client")
}

/// Transfers tokens from a specific private key to an address
///
/// # Arguments
/// * `rpc_url` - The RPC endpoint URL
/// * `from_private_key` - The private key to send from
/// * `amount` - The amount of tokens to send (in wei)
/// * `to_address` - The recipient address
///
/// # Returns
/// * `Result<String>` - The transaction hash as a hex string
pub async fn transfer_token_with_from(
    rpc_url: &str,
    from_private_key: &str,
    amount: U256,
    to_address: &str,
) -> Result<String> {
    let key_str = from_private_key.trim_start_matches("0x");
    let signer = PrivateKeySigner::from_str(key_str)
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();

    let wallet = EthereumWallet::from(signer);

    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url.parse()?);

    let to = Address::from_str(to_address)
        .map_err(|e| eyre!("Failed to parse recipient address: {}", e))?;

    let chain_id =
        provider.get_chain_id().await.map_err(|e| eyre!("Failed to get chain ID: {}", e))?;

    let nonce = provider
        .get_transaction_count(from_address)
        .pending()
        .await
        .map_err(|e| eyre!("Failed to get nonce: {}", e))?;

    println!(
        "Sending transaction: from={:#x}, to={:#x}, amount={}, nonce={}",
        from_address, to, amount, nonce
    );

    let gas_estimate = provider
        .estimate_gas(TransactionRequest::default().from(from_address).to(to).value(amount))
        .await
        .map_err(|e| eyre!("Failed to estimate gas: {}", e))?;

    let gas_price =
        provider.get_gas_price().await.map_err(|e| eyre!("Failed to get gas price: {}", e))?;

    let tx = TransactionRequest::default()
        .with_from(from_address)
        .with_to(to)
        .with_value(amount)
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(gas_estimate)
        .with_gas_price(gas_price);

    let pending_tx = provider
        .send_transaction(tx)
        .await
        .map_err(|e| eyre!("Failed to send transaction: {}", e))?;

    let tx_hash = *pending_tx.tx_hash();
    println!("Transaction sent: {:#x}, waiting for confirmation...", tx_hash);

    // Wait for the transaction to be mined
    let timeout = DEFAULT_TIMEOUT_TX_TO_BE_MINED;
    let receipt = tokio::time::timeout(timeout, pending_tx.get_receipt())
        .await
        .map_err(|_| eyre!("Timeout waiting for transaction {:#x} to be mined", tx_hash))?
        .map_err(|e| eyre!("Failed to get transaction receipt: {}", e))?;

    if !receipt.status() {
        return Err(eyre!("Transaction failed"));
    }

    println!("Transaction {:#x} confirmed successfully", tx_hash);
    Ok(format!("{:#x}", tx_hash))
}

/// Transfers tokens using the default rich private key
///
/// # Arguments
/// * `rpc_url` - The RPC endpoint URL
/// * `amount` - The amount of tokens to send (in wei)
/// * `to_address` - The recipient address
///
/// # Returns
/// * `Result<String>` - The transaction hash as a hex string
pub async fn transfer_token(rpc_url: &str, amount: U256, to_address: &str) -> Result<String> {
    transfer_token_with_from(rpc_url, DEFAULT_RICH_PRIVATE_KEY, amount, to_address).await
}

/// Deploys a smart contract to the blockchain
///
/// # Arguments
/// * `rpc_url` - The RPC endpoint URL
/// * `private_key` - The private key to deploy from
/// * `bytecode_hex` - The contract bytecode as hex string
/// * `constructor_args` - Optional constructor arguments as encoded bytes
///
/// # Returns
/// * `Result<Address>` - The deployed contract address
pub async fn deploy_contract(
    rpc_url: &str,
    private_key: &str,
    bytecode_hex: &str,
    constructor_args: Option<Bytes>,
) -> Result<Address> {
    let signer = PrivateKeySigner::from_str(private_key)?;
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url.parse()?);

    let mut bytecode = hex::decode(bytecode_hex.trim_start_matches("0x"))?;
    if let Some(args) = constructor_args {
        bytecode.extend_from_slice(&args);
    }
    let data = Bytes::from(bytecode);

    let from = signer.address();
    let nonce = provider.get_transaction_count(from).pending().await?;
    let gas_price = provider.get_gas_price().await?;
    let chain_id = provider.get_chain_id().await?;

    let tx = TransactionRequest::default()
        .with_from(from)
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(5_000_000)
        .with_gas_price(gas_price)
        .with_deploy_code(data);

    let pending = provider.send_transaction(tx).await?;
    let receipt = pending.get_receipt().await?;
    if !receipt.status() {
        return Err(eyre!("deployment failed"));
    }

    receipt.contract_address.ok_or_else(|| eyre!("no contract address"))
}

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
    /// Address used for contract deployment
    pub deployment_address: Address,
}

// Global variables to store deployed contract addresses
use std::sync::OnceLock;
static DEPLOYED_CONTRACTS: OnceLock<DeployedContracts> = OnceLock::new();

/// Ensures all test contracts are deployed, deploying them only once
///
/// This function will:
/// 1. Check if contracts are already deployed (cached)
/// 2. Fund the deployment address with 5 ETH
/// 3. Deploy all test contracts (ContractA, ContractB, ContractC, Factory, ERC20)
/// 4. Cache the deployed addresses for future use
///
/// # Returns
/// * `Result<&'static DeployedContracts>` - Reference to the deployed contract addresses
pub async fn ensure_contracts_deployed() -> Result<&'static DeployedContracts> {
    // Return cached contracts if already deployed
    if let Some(contracts) = DEPLOYED_CONTRACTS.get() {
        println!("Contracts already deployed in this test run, returning cached addresses");
        return Ok(contracts);
    }

    use super::{
        constants_xlayer::*,
        manager::{DEFAULT_L2_NETWORK_URL, DEFAULT_RICH_PRIVATE_KEY},
    };
    use alloy_sol_types::SolValue;

    println!("=== Starting contract deployment ===");

    // Get deployment address from TmpSenderPrivateKey
    let deployment_signer = PrivateKeySigner::from_str(TMP_SENDER_PRIVATE_KEY)?;
    let deployment_address = deployment_signer.address();

    println!("Deployment address: {:#x}", deployment_address);

    // Check if deployment address already has sufficient funds
    let provider = ProviderBuilder::new().connect_http(DEFAULT_L2_NETWORK_URL.parse()?);

    let current_balance = provider.get_balance(deployment_address).await?;
    let min_required_balance = U256::from(1_000_000_000_000_000_000u128); // 1 ETH minimum

    if current_balance < min_required_balance {
        // Fund deployment address with 5 ETH
        let funding_amount = U256::from(5_000_000_000_000_000_000u128); // 5 ETH
        println!("Funding deployment address... (current balance: {} wei)", current_balance);
        transfer_token_with_from(
            DEFAULT_L2_NETWORK_URL,
            DEFAULT_RICH_PRIVATE_KEY,
            funding_amount,
            &format!("{:#x}", deployment_address),
        )
        .await?;
    } else {
        println!(
            "Deployment address already has sufficient funds ({} wei), skipping funding",
            current_balance
        );
    }

    println!("\n=== Deploying contracts ===");

    // Deploy ContractB (no constructor args)
    println!("Deploying ContractB...");
    let contract_b = deploy_contract(
        DEFAULT_L2_NETWORK_URL,
        TMP_SENDER_PRIVATE_KEY,
        CONTRACT_B_BYTECODE_STR,
        None,
    )
    .await?;
    println!("ContractB deployed at: {:#x}", contract_b);

    // Deploy ContractA (requires ContractB address as constructor arg)
    println!("Deploying ContractA...");
    let contract_a_constructor_args = contract_b.abi_encode();
    let contract_a = deploy_contract(
        DEFAULT_L2_NETWORK_URL,
        TMP_SENDER_PRIVATE_KEY,
        CONTRACT_A_BYTECODE_STR,
        Some(Bytes::from(contract_a_constructor_args)),
    )
    .await?;
    println!("ContractA deployed at: {:#x}", contract_a);

    // Deploy ContractFactory (no constructor args)
    println!("Deploying ContractFactory...");
    let factory = deploy_contract(
        DEFAULT_L2_NETWORK_URL,
        TMP_SENDER_PRIVATE_KEY,
        CONTRACT_FACTORY_BYTECODE_STR,
        None,
    )
    .await?;
    println!("ContractFactory deployed at: {:#x}", factory);

    // Deploy ContractC (no constructor args)
    println!("Deploying ContractC...");
    let contract_c = deploy_contract(
        DEFAULT_L2_NETWORK_URL,
        TMP_SENDER_PRIVATE_KEY,
        CONTRACT_C_BYTECODE_STR,
        None,
    )
    .await?;
    println!("ContractC deployed at: {:#x}", contract_c);

    // Deploy ERC20 (no constructor args)
    println!("Deploying ERC20...");
    let erc20 =
        deploy_contract(DEFAULT_L2_NETWORK_URL, TMP_SENDER_PRIVATE_KEY, ERC20_BYTECODE_STR, None)
            .await?;
    println!("ERC20 deployed at: {:#x}", erc20);

    // Store deployed contract addresses in global state
    let contracts = DeployedContracts {
        contract_a,
        contract_b,
        contract_c,
        factory,
        erc20,
        deployment_address,
    };

    // Cache the deployed contracts (marks ContractsDeployed = true)
    DEPLOYED_CONTRACTS.set(contracts).map_err(|_| eyre!("Failed to cache deployed contracts"))?;

    println!("\n=== All contracts deployed successfully! ===");
    println!("ContractA: {:#x}", contract_a);
    println!("ContractB: {:#x}", contract_b);
    println!("ContractC: {:#x}", contract_c);
    println!("Factory: {:#x}", factory);
    println!("ERC20: {:#x}", erc20);
    println!("Deployment address: {:#x}", deployment_address);
    println!("ContractsDeployed: true");

    Ok(DEPLOYED_CONTRACTS.get().unwrap())
}

/// Helper function to wait for blocks to be mined
pub async fn wait_for_blocks(client: &jsonrpsee::http_client::HttpClient, min_blocks: u64) -> u64 {
    let mut block_number: u64 = 0;
    for i in 0..30 {
        block_number = super::rpc_all::eth_block_number(client).await.unwrap();
        println!("Block number: {}, attempt {}", block_number, i);
        if block_number > min_blocks {
            break;
        }
        sleep(Duration::from_secs(1)).await;
    }
    block_number
}

/// Sets up the test environment by waiting for blocks and returning a valid block hash and number
///
/// # Arguments
/// * `client` - The JSON-RPC client to use
///
/// # Returns
/// * `Result<(String, u64)>` - A tuple of (block_hash, block_number)
pub async fn setup_test_environment(
    client: &jsonrpsee::http_client::HttpClient,
) -> Result<(String, u64)> {
    // Wait for at least one block to be available
    let block_number = wait_for_blocks(client, 0).await;

    if block_number == 0 {
        return Err(eyre!("Block number should be greater than 0"));
    }

    // Get block data by number
    let block_data = super::rpc_all::eth_get_block_by_number(client, block_number, true).await?;

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

/// Creates and sends an ERC20 transfer transaction
///
/// # Arguments
/// * `rpc_url` - The RPC endpoint URL
/// * `private_key` - The private key to send from
/// * `amount` - The amount of tokens to transfer
/// * `gas_price` - Optional gas price (will use suggested if None)
/// * `to_address` - The recipient address
/// * `erc20_address` - The ERC20 contract address
/// * `nonce` - The transaction nonce
///
/// # Returns
/// * `Result<String>` - The transaction hash
pub async fn erc20_transfer_tx(
    rpc_url: &str,
    private_key: &str,
    amount: U256,
    gas_price: Option<u128>,
    to_address: Address,
    erc20_address: Address,
    nonce: u64,
) -> Result<String> {
    use alloy_sol_types::{sol, SolCall};

    // Define the ERC20 transfer function
    sol! {
        function transfer(address to, uint256 amount) external returns (bool);
    }

    // Parse the private key
    let key_str = private_key.trim_start_matches("0x");
    let signer = PrivateKeySigner::from_str(key_str)?;
    let from_address = signer.address();

    // Create wallet from signer
    let wallet = EthereumWallet::from(signer);

    // Build provider with wallet
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url.parse()?);

    // Get gas price if not provided
    let final_gas_price =
        if let Some(gp) = gas_price { gp } else { provider.get_gas_price().await? };

    // Encode the transfer function call
    let call = transferCall { to: to_address, amount };
    let calldata = call.abi_encode();

    // Get chain ID
    let chain_id = provider.get_chain_id().await?;

    // Build the transaction
    let tx = TransactionRequest::default()
        .with_from(from_address)
        .with_to(erc20_address)
        .with_value(U256::ZERO) // ERC20 transfers don't send ETH
        .with_input(calldata)
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(60000) // Fixed gas limit for ERC20 transfers
        .with_gas_price(final_gas_price);

    // Send the transaction
    let pending_tx = provider.send_transaction(tx).await?;
    let tx_hash = *pending_tx.tx_hash();

    Ok(format!("{:#x}", tx_hash))
}

/// Sends a batch of ERC20 token transfers and waits for them to be mined
///
/// # Arguments
/// * `rpc_url` - The RPC endpoint URL
/// * `erc20_address` - The ERC20 contract address
/// * `amount` - The amount of tokens to transfer per transaction
/// * `to_address` - The recipient address
/// * `batch_size` - Number of transactions to send
///
/// # Returns
/// * `Result<(Vec<String>, u64, String)>` - Tuple of (tx_hashes, block_number, block_hash)
pub async fn transfer_erc20_token_batch(
    rpc_url: &str,
    erc20_address: Address,
    amount: U256,
    to_address: &str,
    batch_size: usize,
) -> Result<(Vec<String>, u64, String)> {
    let mut tx_hashes: Vec<String> = Vec::new();

    // Parse recipient address
    let to = Address::from_str(to_address)?;

    // Create provider to get gas price and nonce
    let key_str = TMP_SENDER_PRIVATE_KEY.trim_start_matches("0x");
    let signer = PrivateKeySigner::from_str(key_str)?;
    let from_address = signer.address();

    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url.parse()?);

    // Get gas price
    let gas_price = provider.get_gas_price().await?;

    // Get starting nonce (including pending transactions)
    let start_nonce = provider.get_transaction_count(from_address).pending().await?;

    println!("Starting batch transfer of {} transactions", batch_size);

    // Send transactions with incrementing nonces (1 to batch_size-1)
    for i in 1..batch_size {
        let nonce = start_nonce + i as u64;
        let tx_hash = erc20_transfer_tx(
            rpc_url,
            TMP_SENDER_PRIVATE_KEY,
            amount,
            Some(gas_price),
            to,
            erc20_address,
            nonce,
        )
        .await?;
        tx_hashes.push(tx_hash);
        println!("Sent transaction {}/{}: {}", i, batch_size - 1, tx_hashes.last().unwrap());
    }

    // Send the last transaction with the starting nonce
    let tx_hash = erc20_transfer_tx(
        rpc_url,
        TMP_SENDER_PRIVATE_KEY,
        amount,
        Some(gas_price),
        to,
        erc20_address,
        start_nonce,
    )
    .await?;
    tx_hashes.push(tx_hash.clone());
    println!("Sent final transaction: {}", tx_hash);

    // Wait for all transactions to be mined
    println!("Waiting for all transactions to be mined...");
    for (i, tx_hash) in tx_hashes.iter().enumerate() {
        wait_for_transaction_receipt(rpc_url, tx_hash, DEFAULT_TIMEOUT_TX_TO_BE_MINED).await?;
        println!("Transaction {}/{} mined", i + 1, tx_hashes.len());
    }

    println!("All {} transactions have been mined successfully", tx_hashes.len());

    // Get receipt of the last transaction
    let last_tx_hash = tx_hashes.last().unwrap();
    let receipt = get_transaction_receipt(rpc_url, last_tx_hash).await?;

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
async fn wait_for_transaction_receipt(
    rpc_url: &str,
    tx_hash: &str,
    timeout: Duration,
) -> Result<()> {
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(eyre!("Timeout waiting for transaction {}", tx_hash));
        }

        match get_transaction_receipt(rpc_url, tx_hash).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Gets a transaction receipt
async fn get_transaction_receipt(rpc_url: &str, tx_hash: &str) -> Result<serde_json::Value> {
    // Create HTTP client
    let client = jsonrpsee::http_client::HttpClientBuilder::default().build(rpc_url)?;

    // Use the eth_get_transaction_receipt function from rpc_all
    let result = super::rpc_all::eth_get_transaction_receipt(&client, tx_hash).await?;

    if result.is_null() {
        return Err(eyre!("Transaction receipt not found"));
    }

    Ok(result)
}

/// Signs, sends, and waits for a transaction to be mined
///
/// # Arguments
/// * `rpc_url` - The RPC endpoint URL
/// * `private_key` - The private key to sign with
/// * `tx_request` - The transaction request to send
///
/// # Returns
/// * `Result<(String, serde_json::Value)>` - Tuple of (tx_hash, receipt)
pub async fn sign_and_send_transaction(
    rpc_url: &str,
    private_key: &str,
    tx_request: TransactionRequest,
) -> Result<(String, serde_json::Value)> {
    let key_str = private_key.trim_start_matches("0x");
    let signer = PrivateKeySigner::from_str(key_str)
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let wallet = EthereumWallet::from(signer);

    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url.parse()?);

    let pending_tx = provider
        .send_transaction(tx_request)
        .await
        .map_err(|e| eyre!("Failed to send transaction: {}", e))?;

    let tx_hash = *pending_tx.tx_hash();
    println!("tx sent: {:#x}", tx_hash);

    wait_for_transaction_receipt(
        rpc_url,
        &format!("{:#x}", tx_hash),
        DEFAULT_TIMEOUT_TX_TO_BE_MINED,
    )
    .await?;

    let receipt = get_transaction_receipt(rpc_url, &format!("{:#x}", tx_hash)).await?;

    let status = receipt["status"].as_str().ok_or_else(|| eyre!("Receipt missing status field"))?;

    if status != "0x1" {
        return Err(eyre!("Transaction failed with status: {}", status));
    }

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
///
/// # Arguments
/// * `trace_result` - The trace result from debug_traceTransaction
/// * `opcode` - The opcode to search for (e.g., "SSTORE", "SELFDESTRUCT")
///
/// # Returns
/// * `u64` - The refund counter value, or 0 if not found
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
