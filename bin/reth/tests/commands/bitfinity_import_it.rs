//!
//! Integration tests for the bitfinity import command.
//! These tests requires a running EVM node or EVM block extractor node at the specified URL.
//!

use super::utils::*;
use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};
use reth_provider::{BlockNumReader, BlockReader, BlockReaderIdExt};
use std::time::Duration;

#[tokio::test]
async fn bitfinity_test_should_import_data_from_evm() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, None).await.unwrap();

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

        assert_eq!(remote_block.hash.unwrap().0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}

#[tokio::test]
async fn bitfinity_test_should_import_with_small_batch_size() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, None).await.unwrap();

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

        assert_eq!(remote_block.hash.unwrap().0, local_block.header.hash_slow().0);
        assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}


#[tokio::test]
async fn bitfinity_test_finalized_and_safe_query_params_works() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, None).await.unwrap();

    let end_block = 100;
    import_data.bitfinity_args.end_block = Some(end_block);
    import_data.bitfinity_args.batch_size = (end_block as usize) * 10;

    // Act
    import_blocks(import_data.clone(), Duration::from_secs(20), true).await;

    let latest_block = import_data.blockchain_db.block_by_number_or_tag(reth_rpc_types::BlockNumberOrTag::Finalized).unwrap().unwrap();
    assert_eq!(end_block, latest_block.number);

    let safe_block = import_data.blockchain_db.block_by_number_or_tag(reth_rpc_types::BlockNumberOrTag::Safe).unwrap().unwrap();
    assert_eq!(end_block, safe_block.number);

}