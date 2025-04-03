use alloy_primitives::{Address, B256};
use eyre::Result;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_e2e_test_utils::testsuite::{
    actions::AssertMineBlock,
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_optimism_chainspec::{OpChainSpecBuilder, OP_MAINNET};
use reth_optimism_node::{OpEngineTypes, OpNode};
use std::{io::Chain, sync::Arc};

#[tokio::test]
async fn test_testsuite_op_assert_mine_block() -> Result<()> {
    reth_tracing::init_test_tracing();

    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            OpChainSpecBuilder::default()
                .chain(OP_MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .build()
                .into(),
        ))
        .with_network(NetworkSetup::single_node());

    let test =
        TestBuilder::new().with_setup(setup).with_action(AssertMineBlock::<OpEngineTypes>::new(
            0,
            vec![],
            Some(B256::ZERO),
            OpPayloadAttributes {
                payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    prev_randao: B256::random(),
                    suggested_fee_recipient: Address::random(),
                    withdrawals: None,
                    parent_beacon_block_root: None,
                },
                transactions: None,
                no_tx_pool: None,
                eip_1559_params: None,
                gas_limit: Some(30_000_000),
            },
        ));

    test.run::<OpNode>().await?;

    Ok(())
}

#[tokio::test]
async fn test_testsuite_op_assert_mine_block_isthmus() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Set up a timestamp that's in the future to ensure all hardforks are active
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() +
            3600; // Add an hour to ensure it's in the future

    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            OpChainSpecBuilder::default()
                .chain(Chain::dev())
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .isthmus_activated() // Enable Isthmus at genesis
                .build()
                .into(),
        ))
        .with_network(NetworkSetup::single_node());

    let withdrawals = Some(vec![]);

    let test =
        TestBuilder::new().with_setup(setup).with_action(AssertMineBlock::<OpEngineTypes>::new(
            0,
            vec![],
            Some(B256::ZERO),
            OpPayloadAttributes {
                payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                    timestamp,
                    prev_randao: B256::random(),
                    suggested_fee_recipient: Address::random(),
                    withdrawals, // Include the withdrawals field (empty vector)
                    parent_beacon_block_root: None,
                },
                transactions: None,
                no_tx_pool: None,
                eip_1559_params: None,
                gas_limit: Some(30_000_000),
            },
        ));

    test.run::<OpNode>().await?;

    Ok(())
}

#[tokio::test]
async fn test_testsuite_op_assert_mine_block_isthmus_mainnet() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Set up a timestamp that's in the future to ensure all hardforks are active
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() +
            3600; // Add an hour to ensure it's in the future

    // Create a mainnet chainspec with Isthmus activated at genesis
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            OpChainSpecBuilder::default()
                .chain(OP_MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .isthmus_activated() // Enable Isthmus at genesis
                .build()
                .into(),
        ))
        .with_network(NetworkSetup::single_node());

    let withdrawals = Some(vec![]);

    let test =
        TestBuilder::new().with_setup(setup).with_action(AssertMineBlock::<OpEngineTypes>::new(
            0,
            vec![],
            Some(B256::ZERO),
            OpPayloadAttributes {
                payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                    timestamp,
                    prev_randao: B256::random(),
                    suggested_fee_recipient: Address::random(),
                    withdrawals, // Include the withdrawals field (empty vector)
                    parent_beacon_block_root: None,
                },
                transactions: None,
                no_tx_pool: None,
                eip_1559_params: None,
                gas_limit: Some(30_000_000),
            },
        ));

    test.run::<OpNode>().await?;

    Ok(())
}

#[test]
fn test_genesis_upgrade_with_proxy_implementation() {
    // Define addresses for proxy and implementation contracts
    let proxy_address = Address::from([0x42; 20]);
    let implementation_address = Address::from([0x43; 20]);

    // Simple proxy contract bytecode that delegates calls to implementation
    // This is a basic proxy pattern with DELEGATECALL to implementation
    let proxy_bytecode = Bytecode::new_raw(hex!("363d3d37363d73000000000000000000000000000000000000000000000000000000000000000073000000000000000000000000000000000000000000000000000000000000000061010036515af43d3d93803e602a57fd5bf3").to_vec());

    // Simple implementation contract with some storage slots
    let implementation_bytecode = Bytecode::new_raw(hex!("60806040526004361061001e5760003560e01c80635c60da1b14610023575b600080fd5b61002b61004d565b604051610038919061013b565b60405180910390f35b60008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff16905090565b600073ffffffffffffffffffffffffffffffffffffffff82169050919050565b6000610105826100da565b9050919050565b61011581610100565b82525050565b600060208201905061013060008301846100f6565b92915050565b60006020820190506101356000830184610156565b9291505056").to_vec());

    // Create initial state with accounts and storage
    let mut state = HashedPostState::default();

    // Create proxy account
    let proxy_account = AccountInfo {
        balance: U256::ZERO,
        code_hash: keccak256(proxy_bytecode.clone()),
        code: Some(proxy_bytecode),
        nonce: 1,
    };

    // Create implementation account
    let implementation_account = AccountInfo {
        balance: U256::ZERO,
        code_hash: keccak256(implementation_bytecode.clone()),
        code: Some(implementation_bytecode),
        nonce: 1,
    };

    // Add accounts to state
    state.accounts.insert(keccak256(proxy_address), proxy_account);
    state.accounts.insert(keccak256(implementation_address), implementation_account);

    // Set up storage for proxy - including implementation address at slot 0
    // which is a common pattern for proxy contracts
    let mut proxy_storage = HashedStorage::default();
    // Store implementation address at slot 0
    proxy_storage.insert(B256::ZERO, U256::from_be_bytes(implementation_address.into_word()));
    // Add some additional storage values to test changes
    proxy_storage.insert(
        B256::from_str("0x1000000000000000000000000000000000000000000000000000000000000000")
            .unwrap(),
        U256::from(42),
    );

    // Add storage to state
    state.storages.insert(keccak256(proxy_address), proxy_storage.clone());

    // Initialize test database with a chain spec
    let chain_spec = Arc::new(
        ChainSpecBuilder::default().chain(Chain::dev()).genesis(Default::default()).build(),
    );
    let provider_factory = create_test_provider_factory_with_node_types::<EthNode>(chain_spec);

    // Initialize genesis
    let _ = init_genesis(&provider_factory).unwrap();

    // Write initial state to database
    let provider_rw = provider_factory.provider_rw().unwrap();
    provider_rw.write_hashed_state(&state.clone().into_sorted()).unwrap();
    provider_rw.commit().unwrap();

    // Calculate initial storage root for proxy contract
    let initial_storage_root = storage_root_prehashed(proxy_storage.storage);

    // Create a modified storage for the upgrade
    let mut upgraded_proxy_storage = proxy_storage.clone();
    // Update a storage value to trigger a root change
    upgraded_proxy_storage.insert(
        B256::from_str("0x1000000000000000000000000000000000000000000000000000000000000000")
            .unwrap(),
        U256::from(100), // Changed from 42 to 100
    );
    // Add a new storage slot
    upgraded_proxy_storage.insert(
        B256::from_str("0x2000000000000000000000000000000000000000000000000000000000000000")
            .unwrap(),
        U256::from(200),
    );

    // Calculate new storage root after changes
    let upgraded_storage_root = storage_root_prehashed(upgraded_proxy_storage.storage);

    // Verify the storage root has changed
    assert_ne!(
        initial_storage_root, upgraded_storage_root,
        "Storage root should change after upgrade"
    );

    // Create state update to represent the upgrade
    let mut state_updates = HashedPostState::default();
    state_updates.storages.insert(keccak256(proxy_address), upgraded_proxy_storage);

    // Apply state updates
    let provider_rw = provider_factory.provider_rw().unwrap();
    provider_rw.write_hashed_state(&state_updates.into_sorted()).unwrap();
    provider_rw.commit().unwrap();

    // Read back the storage and verify the changes
    let provider = provider_factory.provider().unwrap();

    // Verify the changes by reading storage at the updated slot
    let value = provider
        .storage(
            proxy_address,
            U256::from(0x1000000000000000000000000000000000000000000000000000000000000000u128),
        )
        .unwrap();
    assert_eq!(value, U256::from(100), "Storage value should be updated");

    // Verify the new storage slot
    let value = provider
        .storage(
            proxy_address,
            U256::from(0x2000000000000000000000000000000000000000000000000000000000000000u128),
        )
        .unwrap();
    assert_eq!(value, U256::from(200), "New storage slot should be added");

    // Verify implementation address is still correctly stored
    let value = provider.storage(proxy_address, U256::ZERO).unwrap();
    assert_eq!(
        value,
        U256::from_be_bytes(implementation_address.into_word()),
        "Implementation address should be preserved"
    );

    // Create a block header that would use the new storage root
    let header = Header {
        number: 1,
        state_root: state_root_unhashed(provider.hashed_state_with_metadata()).unwrap(),
        ..Default::default()
    };

    // Create a blockchain provider to validate the block
    let blockchain_provider = BlockchainProvider::new(provider_factory).unwrap();

    // Simple validation that the state root matches
    let calculated_state_root = blockchain_provider.state_root(state_updates).unwrap();
    assert_eq!(
        header.state_root, calculated_state_root,
        "State root in header should match calculated state root"
    );

    println!("Genesis upgrade with proxy and implementation contracts successfully tested!");
}
