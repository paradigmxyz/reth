use alloy_chains::Chain;
use alloy_consensus::Header;
use alloy_primitives::{hex, keccak256, Address, Bytes, B256, U256};
use alloy_rpc_types_eth::AccountInfo;
use eyre::Result;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_chainspec::ChainSpecBuilder;
use reth_e2e_test_utils::testsuite::{
    actions::AssertMineBlock,
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_node_core::primitives::{Account, Bytecode};
use reth_optimism_chainspec::{OpChainSpecBuilder, OP_MAINNET};
use reth_optimism_node::{OpEngineTypes, OpNode};
use reth_provider::{
    providers::BlockchainProvider, test_utils::create_test_provider_factory_with_node_types,
};
use reth_trie_common::{root::state_root_unhashed, HashedPostState, HashedStorage};
use std::{convert::TryFrom, str::FromStr, sync::Arc};

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

/// Test the storage contract implementation mechanism
#[test]
fn test_simple_proxy_implementation() {
    // Define test addresses
    let proxy_address = Address::from([0x42; 20]);
    let implementation_address = Address::from([0x43; 20]);

    // Create a simple test bytecode for the proxy
    let proxy_bytecode_bytes = hex!("363d3d37363d73").to_vec();
    // Add the implementation address placeholder
    let proxy_bytecode = Bytecode::new_raw(Bytes::from(proxy_bytecode_bytes));

    // Create a simple test bytecode for the implementation
    let implementation_bytecode_bytes = hex!("6080604052").to_vec();
    let implementation_bytecode = Bytecode::new_raw(Bytes::from(implementation_bytecode_bytes));

    // Create a test state with accounts and storage
    let mut state = HashedPostState::default();

    // Set up the proxy account - fixed field names
    let proxy_account = Account {
        balance: U256::ZERO,
        bytecode_hash: Some(keccak256(proxy_bytecode.bytecode())),
        nonce: 1,
    };

    // Set up the implementation account
    let implementation_account = Account {
        balance: U256::ZERO,
        bytecode_hash: Some(keccak256(implementation_bytecode.bytecode())),
        nonce: 1,
    };

    // Add accounts to state - wrap accounts in Some()
    state.accounts.insert(keccak256(proxy_address), Some(proxy_account));
    state.accounts.insert(keccak256(implementation_address), Some(implementation_account));

    // Set up storage for the proxy, pointing to implementation
    let mut proxy_storage = HashedStorage::default();

    // Storage slot 0 typically contains the implementation address in proxy patterns
    let impl_bytes: [u8; 32] = implementation_address.into_word().into();
    proxy_storage.storage.insert(B256::ZERO, U256::from_be_bytes(impl_bytes));

    // Add a test value in storage
    let test_slot =
        B256::from_slice(&hex!("0100000000000000000000000000000000000000000000000000000000000000"));
    proxy_storage.storage.insert(test_slot, U256::from(42));

    // Add storage to state
    state.storages.insert(keccak256(proxy_address), proxy_storage.clone());

    // Create a modified version of the proxy storage that would be used in an upgrade
    let mut upgraded_storage = HashedStorage::default();

    // Keep the implementation address
    upgraded_storage.storage.insert(B256::ZERO, U256::from_be_bytes(impl_bytes));

    // Update the test value
    upgraded_storage.storage.insert(test_slot, U256::from(100));

    // Fix state_root_unhashed calls to use Address instead of B256
    let initial_root =
        state_root_unhashed(std::iter::once((proxy_address, proxy_storage.storage.clone())));

    let upgraded_root =
        state_root_unhashed(std::iter::once((proxy_address, upgraded_storage.storage.clone())));

    // Verify the roots are different
    assert!(initial_root != upgraded_root, "Storage roots should be different after upgrade");

    println!("Proxy implementation test completed successfully");
}
