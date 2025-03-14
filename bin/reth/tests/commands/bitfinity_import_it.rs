//!
//! Integration tests for the bitfinity import command.
//! These tests requires a running EVM node or EVM block extractor node at the specified URL.

use crate::commands::bitfinity_node_it::eth_server::BfEvmServer;

use super::{
    bitfinity_node_it::{
        eth_server::{EthImpl, EthServer},
        mock_eth_server_start, mock_multi_server_start,
    },
    utils::*,
};
use alloy_eips::BlockNumberOrTag;

use did::H256;
use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};

use reth::commands::bitfinity_import::BitfinityImportCommand;
use reth_provider::{BlockNumReader, BlockReader, BlockReaderIdExt};
use reth_trie::StateRoot;
use reth_trie_db::DatabaseStateRoot;
use revm_primitives::{Address, U256};

use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn bitfinity_test_should_import_data_from_evm() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, None, None).await.unwrap();

    let end_block = 100;
    import_data.bitfinity_args.end_block = Some(end_block);
    import_data.bitfinity_args.batch_size = (end_block as usize) * 10;

    // Act
    import_blocks(import_data.clone(), Duration::from_secs(20), false).await;

    // Assert
    {
        let provider = import_data.provider_factory.provider().unwrap();
        assert_eq!(end_block, provider.last_block_number().unwrap());

        // create evm client
        let evm_rpc_client =
            EthJsonRpcClient::new(ReqwestClient::new(evm_datasource_url.to_string()));

        let remote_block = evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
        let local_block = provider.block_by_number(end_block).unwrap().unwrap();

        assert_eq!(remote_block.hash.0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}

#[tokio::test]
async fn bitfinity_test_should_import_with_small_batch_size() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, None, None).await.unwrap();

    let end_block = 101;
    import_data.bitfinity_args.end_block = Some(end_block);
    import_data.bitfinity_args.batch_size = 10;

    // Act
    import_blocks(import_data.clone(), Duration::from_secs(20), false).await;

    // Assert
    {
        let provider = import_data.provider_factory.provider().unwrap();
        assert_eq!(end_block, provider.last_block_number().unwrap());

        // create evm client
        let evm_rpc_client =
            EthJsonRpcClient::new(ReqwestClient::new(evm_datasource_url.to_string()));

        let remote_block = evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
        let local_block = provider.block_by_number(end_block).unwrap().unwrap();

        assert_eq!(remote_block.hash.0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}

#[tokio::test]
async fn bitfinity_test_finalized_and_safe_query_params_works() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, None, None).await.unwrap();

    let end_block = 100;
    import_data.bitfinity_args.end_block = Some(end_block);
    import_data.bitfinity_args.batch_size = (end_block as usize) * 10;

    // Act
    import_blocks(import_data.clone(), Duration::from_secs(20), true).await;

    let latest_block = import_data
        .blockchain_db
        .block_by_number_or_tag(BlockNumberOrTag::Finalized)
        .unwrap()
        .unwrap();
    assert_eq!(end_block, latest_block.number);

    let safe_block =
        import_data.blockchain_db.block_by_number_or_tag(BlockNumberOrTag::Safe).unwrap().unwrap();
    assert_eq!(end_block, safe_block.number);
}

#[tokio::test]
async fn bitfinity_test_should_import_data_from_evm_with_backup_rpc_url() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = "https://fake_rpc_url";
    let backup_rpc_url = DEFAULT_EVM_DATASOURCE_URL;

    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, Some(backup_rpc_url.to_owned()), None)
            .await
            .unwrap();

    let end_block = 100;
    import_data.bitfinity_args.end_block = Some(end_block);
    import_data.bitfinity_args.batch_size = (end_block as usize) * 10;

    // Act
    import_blocks(import_data.clone(), Duration::from_secs(200), false).await;

    // Assert
    {
        let provider = import_data.provider_factory.provider().unwrap();
        assert_eq!(end_block, provider.last_block_number().unwrap());

        // create evm client
        let evm_rpc_client = EthJsonRpcClient::new(ReqwestClient::new(backup_rpc_url.to_string()));

        let remote_block = evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
        let local_block = provider.block_by_number(end_block).unwrap().unwrap();

        assert_eq!(remote_block.hash.0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}

#[tokio::test]
async fn bitfinity_test_should_not_import_blocks_when_evm_is_disabled_and_check_evm_state_before_importing_is_true(
) {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::with_evm_state(did::evm_state::EvmGlobalState::Disabled);
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();
    import_data.bitfinity_args.end_block = Some(10);
    import_data.bitfinity_args.batch_size = 5;
    import_data.bitfinity_args.max_fetch_blocks = 10;
    import_data.bitfinity_args.check_evm_state_before_importing = true;

    // Act - Try to import blocks
    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert - No blocks should have been imported
    let provider = import_data.provider_factory.provider().unwrap();
    let last_block = provider.last_block_number().unwrap();
    assert_eq!(last_block, 0, "Expected no blocks to be imported when staging mode is disabled");
}

#[tokio::test]
async fn bitfinity_test_should_import_blocks_when_evm_is_enabled_and_check_evm_state_before_importing_is_true(
) {
    // Arrange
    let _log = init_logs();

    // Set up mock ETH server in disabled mode
    let eth_server = EthImpl::with_evm_state(did::evm_state::EvmGlobalState::Enabled);
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();
    //
    // Wait for enough blocks to be minted
    tokio::time::sleep(Duration::from_secs(1)).await;

    import_data.bitfinity_args.end_block = Some(10);
    import_data.bitfinity_args.batch_size = 5;
    import_data.bitfinity_args.max_fetch_blocks = 10;
    import_data.bitfinity_args.check_evm_state_before_importing = true;

    // Act - Try to import blocks
    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert
    let provider = import_data.provider_factory.provider().unwrap();
    let last_block = provider.last_block_number().unwrap();
    assert_eq!(last_block, 10, "Expected 10 blocks to be imported when EVM is enabled");
}

#[tokio::test]
async fn bitfinity_test_should_import_block_when_evm_is_enabled_and_check_evm_state_before_importing_is_false(
) {
    // Arrange
    let _log = init_logs();

    // Act
    // Set up mock ETH server in disabled mode
    let eth_server = EthImpl::with_evm_state(did::evm_state::EvmGlobalState::Enabled);
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();

    // Wait for enough blocks to be minted
    tokio::time::sleep(Duration::from_secs(1)).await;

    import_data.bitfinity_args.end_block = Some(10);
    import_data.bitfinity_args.batch_size = 5;
    import_data.bitfinity_args.max_fetch_blocks = 10;
    import_data.bitfinity_args.check_evm_state_before_importing = false;

    // Act - Try to import blocks
    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert
    let provider = import_data.provider_factory.provider().unwrap();
    let last_block = provider.last_block_number().unwrap();
    assert_eq!(last_block, 10, "Expected 10 blocks to be imported when EVM is enabled");
}

#[tokio::test]
async fn bitfinity_test_should_not_import_block_when_evm_is_staging_and_check_evm_state_before_importing_is_false(
) {
    // Arrange
    let _log = init_logs();

    // Set up mock ETH server in disabled mode
    let eth_server =
        EthImpl::with_evm_state(did::evm_state::EvmGlobalState::Staging { max_block_number: None });
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    // Act
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();

    import_data.bitfinity_args.end_block = Some(10);
    import_data.bitfinity_args.batch_size = 5;
    import_data.bitfinity_args.max_fetch_blocks = 10;
    import_data.bitfinity_args.check_evm_state_before_importing = true;

    // Act
    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert
    let provider = import_data.provider_factory.provider().unwrap();
    let last_block = provider.last_block_number().unwrap();
    assert_eq!(last_block, 0, "Expected no blocks to be imported when EVM is staging");
}

#[tokio::test]
async fn bitfinity_test_should_import_data_to_safe_block() {
    // Arrange
    let _log = init_logs();

    const UNSAFE_BLOCKS: u64 = 3;
    const MAX_BLOCKS: u64 = 10;

    let mut eth_server = EthImpl::new_with_max_block(MAX_BLOCKS);
    let bf_evm_server = eth_server.bf_impl(UNSAFE_BLOCKS);

    let (_server, eth_server_address) = mock_multi_server_start([
        EthServer::into_rpc(eth_server).into(),
        BfEvmServer::into_rpc(bf_evm_server).into(),
    ])
    .await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    // Wait for blocks to be minted
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Act
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();

    import_data.bitfinity_args.max_fetch_blocks = 100;

    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    let safe_block = MAX_BLOCKS - UNSAFE_BLOCKS;

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert
    let provider = import_data.provider_factory.provider().unwrap();
    let last_imported_block = provider.last_block_number().unwrap();
    assert_eq!(last_imported_block, safe_block);
}

#[tokio::test]
async fn bitfinity_test_should_confirm_and_import_unsafe_blocks() {
    // Arrange
    let _log = init_logs();

    const UNSAFE_BLOCKS: u64 = 3;
    const MAX_BLOCKS: u64 = 10;

    let genesis_balances =
        vec![(Address::from_slice(&[0u8; 20]).into(), U256::from(1_000_000_u64))];

    let state_root = compute_state_root(&genesis_balances);

    let mut eth_server = EthImpl::new_with_max_block(MAX_BLOCKS)
        .with_genesis_balances(genesis_balances)
        .with_state_root(state_root.into());

    let bf_evm_server = eth_server.bf_impl(UNSAFE_BLOCKS);

    let (_server, eth_server_address) = mock_multi_server_start([
        EthServer::into_rpc(eth_server).into(),
        BfEvmServer::into_rpc(bf_evm_server).into(),
    ])
    .await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    // Wait for blocks to be minted
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Act
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();

    import_data.bitfinity_args.max_fetch_blocks = 100;
    import_data.bitfinity_args.confirm_unsafe_blocks = true;

    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert
    let provider = import_data.provider_factory.provider().unwrap();
    let last_imported_block = provider.last_block_number().unwrap();
    assert_eq!(last_imported_block, MAX_BLOCKS);
}

#[tokio::test]
async fn bitfinity_test_should_import_until_last_confirmed() {
    // Arrange
    let _log = init_logs();

    const UNSAFE_BLOCKS: u64 = 3;
    const MAX_BLOCKS: u64 = 10;
    const CONFIRM_UNTIL: u64 = 8;
    let genesis_balances =
        vec![(Address::from_slice(&[0u8; 20]).into(), U256::from(1_000_000_u64))];

    let state_root = compute_state_root(&genesis_balances);

    let mut eth_server = EthImpl::new_with_max_block(MAX_BLOCKS)
        .with_genesis_balances(genesis_balances)
        .with_state_root(state_root.into());
    let mut bf_evm_server = eth_server.bf_impl(UNSAFE_BLOCKS);
    bf_evm_server.confirm_until = CONFIRM_UNTIL;

    let (_server, eth_server_address) = mock_multi_server_start([
        EthServer::into_rpc(eth_server).into(),
        BfEvmServer::into_rpc(bf_evm_server).into(),
    ])
    .await;
    let evm_datasource_url = format!("http://{}", eth_server_address);

    // Wait for blocks to be minted
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Act
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(&evm_datasource_url, None, None).await.unwrap();

    import_data.bitfinity_args.max_fetch_blocks = 100;
    import_data.bitfinity_args.confirm_unsafe_blocks = true;

    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();

    // Allow some time for potential imports
    tokio::time::sleep(Duration::from_secs(2)).await;
    job_executor.stop(true).await.unwrap();

    // Assert
    let provider = import_data.provider_factory.provider().unwrap();
    let last_imported_block = provider.last_block_number().unwrap();
    assert_eq!(last_imported_block, CONFIRM_UNTIL);
}

/// Compute state root for genesis Balances
fn compute_state_root(genesis_balances: &[(Address, U256)]) -> H256 {
    use alloy_genesis::Genesis;
    use alloy_genesis::GenesisAccount;
    use reth_chainspec::ChainSpec;
    use reth_db_common::init::init_genesis;
    use reth_provider::test_utils::create_test_provider_factory_with_chain_spec;

    let chain_spec = Arc::new(ChainSpec {
        chain: 1_u64.into(),
        genesis: Genesis {
            alloc: genesis_balances
                .iter()
                .map(|(address, balance)| {
                    (*address, GenesisAccount { balance: *balance, ..Default::default() })
                })
                .collect(),
            ..Default::default()
        },
        hardforks: Default::default(),
        genesis_hash: Default::default(),
        paris_block_and_final_difficulty: None,
        deposit_contract: None,
        ..Default::default()
    });

    let factory = create_test_provider_factory_with_chain_spec(chain_spec);
    init_genesis(&factory).unwrap();

    let provider = factory.provider().unwrap();

    let tx = provider.tx_ref();

    StateRoot::from_tx(tx).root().unwrap().into()
}
