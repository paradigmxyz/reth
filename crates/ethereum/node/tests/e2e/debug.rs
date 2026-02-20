use crate::utils::eth_payload_attributes;
use alloy_eip7928::BlockAccessList;
use alloy_network::TransactionBuilder;
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::{network::EthereumWallet, Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_sol_types::SolCall;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

alloy_sol_types::sol! {
    #[sol(rpc, bytecode = "6080604052348015600e575f5ffd5b506040516101a43803806101a4833981016040819052602b916047565b5f55605d565b5f60208284031215605657600080fd5b5051919050565b61013a8061006a5f395ff3fe6080604052348015600e575f5ffd5b50600436106030575f3560e01c80632e64cec11460345780636057361d146048575b5f5ffd5b5f5460405190815260200160405180910390f35b6058605336600460d0565b605a565b005b5f819055604051819081527f9e71bc8eea02a63969f509818f2dafb9254532904319f9dbda79b67bd34a5f3d9060200160405180910390a150565b5f602082840312156094575f5ffd5b813567ffffffffffffffff81111560a9575f5ffd5b82016020818503121560b9575f5ffd5b9392505050565b634e487b7160e01b5f52604160045260245ffd5b5f6020828403121560e0575f5ffd5b503591905056fea164736f6c634300081c000a")]
    contract Storage {
        uint256 value;

        event ValueChanged(uint256 newValue);

        constructor(uint256 initialValue) {
            value = initialValue;
        }

        function retrieve() public view returns (uint256) {
            return value;
        }

        function store(uint256 newValue) public {
            value = newValue;
            emit ValueChanged(newValue);
        }
    }
}

#[tokio::test]
async fn test_debug_get_block_access_list() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let signer = wallet.wallet_gen().swap_remove(0);
    let sender_address = signer.address();
    let provider =
        ProviderBuilder::new().wallet(EthereumWallet::new(signer)).connect_http(node.rpc_url());

    // Deploy a contract that writes to storage
    let builder = Storage::deploy_builder(&provider, U256::from(42)).send().await?;
    node.advance_block().await?;
    let receipt = builder.get_receipt().await?;
    assert!(receipt.status());
    let contract_address = receipt.contract_address.expect("contract deployed");

    // Call the contract's store function to write to storage
    let store_call = Storage::storeCall { newValue: U256::from(100) };
    let tx = TransactionRequest::default()
        .to(contract_address)
        .with_input(Bytes::from(store_call.abi_encode()));
    let pending = provider.send_transaction(tx).await?;
    node.advance_block().await?;
    let store_receipt = pending.get_receipt().await?;
    assert!(store_receipt.status());

    // Get the block number where the store transaction was included
    let block_number = store_receipt.block_number.unwrap();

    // Now call debug_getBlockAccessList for that block
    let bal: BlockAccessList =
        provider.client().request("debug_getBlockAccessList", (block_number,)).await?;

    // Verify the BAL contains the expected data
    assert!(!bal.is_empty(), "BAL should not be empty");

    // Find the sender account in the BAL
    let sender_entry = bal.iter().find(|acc| acc.address == sender_address);
    assert!(sender_entry.is_some(), "Sender should be in BAL");

    // Sender should have balance/nonce changes
    let sender_changes = sender_entry.unwrap();
    assert!(!sender_changes.balance_changes.is_empty(), "Sender should have balance changes");
    assert!(!sender_changes.nonce_changes.is_empty(), "Sender should have nonce changes");

    // Find the contract address in the BAL
    let contract_entry = bal.iter().find(|acc| acc.address == contract_address);
    assert!(contract_entry.is_some(), "Contract should be in BAL");

    // Contract should have storage changes
    let contract_changes = contract_entry.unwrap();
    assert!(!contract_changes.storage_changes.is_empty(), "Contract should have storage changes");

    // Verify the storage change contains the correct value (100)
    let slot_changes = &contract_changes.storage_changes[0];
    assert!(!slot_changes.changes.is_empty(), "Should have at least one storage change");
    assert_eq!(
        slot_changes.changes.last().unwrap().new_value,
        U256::from(100),
        "Storage should be set to 100"
    );

    Ok(())
}

#[tokio::test]
async fn test_debug_get_block_access_list_empty_block() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, _wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new().connect_http(node.rpc_url());

    // Mine an empty block
    node.advance_block().await?;

    // Get BAL for block 1 (empty block)
    let bal: BlockAccessList =
        provider.client().request("debug_getBlockAccessList", (1u64,)).await?;

    // Empty block should return empty BAL
    assert!(bal.is_empty(), "Empty block should have empty BAL");

    Ok(())
}

#[tokio::test]
async fn test_debug_get_block_access_list_multiple_txs() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let signer = wallet.wallet_gen().swap_remove(0);
    let provider =
        ProviderBuilder::new().wallet(EthereumWallet::new(signer)).connect_http(node.rpc_url());

    // Send multiple transactions to different addresses
    let recipient1 = Address::repeat_byte(0x11);
    let recipient2 = Address::repeat_byte(0x22);

    let tx1 = TransactionRequest::default().to(recipient1).with_value(U256::from(1000));
    let tx2 = TransactionRequest::default().to(recipient2).with_value(U256::from(2000));

    let pending1 = provider.send_transaction(tx1).await?;
    let pending2 = provider.send_transaction(tx2).await?;
    node.advance_block().await?;

    let receipt1 = pending1.get_receipt().await?;
    let receipt2 = pending2.get_receipt().await?;
    assert!(receipt1.status());
    assert!(receipt2.status());

    let block_number = receipt1.block_number.unwrap();

    // Get BAL for the block
    let bal: BlockAccessList =
        provider.client().request("debug_getBlockAccessList", (block_number,)).await?;

    // Should have entries for sender and both recipients
    assert!(bal.len() >= 3, "BAL should contain at least sender and two recipients");

    // Both recipients should be in the BAL
    assert!(bal.iter().any(|acc| acc.address == recipient1), "Recipient1 should be in BAL");
    assert!(bal.iter().any(|acc| acc.address == recipient2), "Recipient2 should be in BAL");

    // Recipients should have balance changes
    let recipient1_entry = bal.iter().find(|acc| acc.address == recipient1).unwrap();
    let recipient2_entry = bal.iter().find(|acc| acc.address == recipient2).unwrap();
    assert!(!recipient1_entry.balance_changes.is_empty(), "Recipient1 should have balance changes");
    assert!(!recipient2_entry.balance_changes.is_empty(), "Recipient2 should have balance changes");

    Ok(())
}
