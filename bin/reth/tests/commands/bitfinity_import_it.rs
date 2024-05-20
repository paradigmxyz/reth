//!
//! Integration tests for the bitfinity import command.
//! These tests requires a running EVM node or EVM block extractor node at the specified URL.
//! 

use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};
use reth::commands::bitfinity_import::BitfinityImportCommand;
use reth_provider::{BlockNumReader, BlockReader};
use std::time::Duration;
use super::utils::*;

#[tokio::test]
async fn bitfinity_test_should_import_data_from_evm() {
    // Arrange
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, import_data) = bitfinity_import_config_data(evm_datasource_url).await.unwrap();

    let mut bitfinity = import_data.bitfinity_args;
    let end_block = 100;
    bitfinity.end_block = Some(end_block);
    bitfinity.batch_size = (end_block as usize) * 10;

    // Act
    {
        let import = BitfinityImportCommand::new(
            None,
            import_data.data_dir,
            import_data.chain.clone(),
            bitfinity,
            import_data.provider_factory.clone(),
            import_data.blockchain_db,
        );
        let _import_handle = import.schedule_execution().await.unwrap();
        wait_until_local_block_imported(&import_data.provider_factory, end_block, Duration::from_secs(20)).await;
    }

    // Assert
    {
        let provider = import_data.provider_factory.provider().unwrap();
        assert_eq!(end_block, provider.last_block_number().unwrap());

        // create evm client
        let evm_rpc_client =
            EthJsonRpcClient::new(ReqwestClient::new(evm_datasource_url.to_string()));

        let remote_block = evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
        let local_block = provider.block_by_number(end_block).unwrap().unwrap();

        assert_eq!(remote_block.hash.unwrap().0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}


#[tokio::test]
async fn bitfinity_test_should_import_with_small_batch_size() {
    // Arrange
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, import_data) = bitfinity_import_config_data(evm_datasource_url).await.unwrap();

    let mut bitfinity = import_data.bitfinity_args;
    let end_block = 101;
    bitfinity.end_block = Some(end_block);
    bitfinity.batch_size = 10;

    // Act
    {
        let import = BitfinityImportCommand::new(
            None,
            import_data.data_dir,
            import_data.chain.clone(),
            bitfinity,
            import_data.provider_factory.clone(),
            import_data.blockchain_db,
        );
        let _import_handle = import.schedule_execution().await.unwrap();
        wait_until_local_block_imported(&import_data.provider_factory, end_block, Duration::from_secs(20)).await;
    }

    // Assert
    {
        let provider = import_data.provider_factory.provider().unwrap();
        assert_eq!(end_block, provider.last_block_number().unwrap());

        // create evm client
        let evm_rpc_client =
            EthJsonRpcClient::new(ReqwestClient::new(evm_datasource_url.to_string()));

        let remote_block = evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
        let local_block = provider.block_by_number(end_block).unwrap().unwrap();

        assert_eq!(remote_block.hash.unwrap().0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}
