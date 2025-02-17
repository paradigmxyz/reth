//!
//! Integration tests for the bitfinity import command.
//! These tests requires a running EVM node or EVM block extractor node at the specified URL.
//!

use super::bitfinity_node_it::{
    eth_server::{EthImpl, EthServer},
    mock_eth_server_start,
};
use super::utils::*;
use alloy_eips::BlockNumberOrTag;

use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};

use reth::commands::bitfinity_import::BitfinityImportCommand;
use reth_provider::{BlockNumReader, BlockReader, BlockReaderIdExt};

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

        let remote_block =
            evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
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
