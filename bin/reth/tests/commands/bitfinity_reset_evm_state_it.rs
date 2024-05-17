/// 
/// These are integration tests for the BitfinityResetEvmStateCommand.
/// These tests requires a running EVM node or EVM block extractor node at the specified URL
/// and a running EVM canister on a local dfx node.

use std::{path::PathBuf, time::Duration};

use reth::{args::BitfinityResetEvmStateArgs, commands::{bitfinity_import::BitfinityImportCommand, bitfinity_reset_evm_state::BitfinityResetEvmStateCommand}};

use super::utils::*;

#[tokio::test]
async fn bitfinity_test_should_reset_evm_state() {
    // Arrange
    let dfx_port = get_dfx_local_port();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let temp_data = bitfinity_import_config_data(evm_datasource_url).await.unwrap();

    let end_block = 500;
    
    {
        let mut bitfinity_import_args = temp_data.bitfinity_args;
        bitfinity_import_args.end_block = Some(end_block);
        let import = BitfinityImportCommand::new(
            None,
            temp_data.data_dir.clone(),
            temp_data.chain.clone(),
            bitfinity_import_args,
            temp_data.provider_factory.clone(),
            temp_data.blockchain_db,
        );
        let _import_handle = import.execute().await.unwrap();
        wait_until_local_block_imported(&temp_data.provider_factory, end_block, Duration::from_secs(20)).await;
    }

    let bitfinity_reset_evm_state_command = build_bitfinity_reset_evm_command("alice", temp_data.data_dir.data_dir().to_owned(), dfx_port, evm_datasource_url);

    // Act
    {
        bitfinity_reset_evm_state_command.execute().await.unwrap();
    }

    // Assert
    {
        // let provider = temp_data.provider_factory.provider().unwrap();
        // assert_eq!(end_block, provider.last_block_number().unwrap());

        // // create evm client
        // let evm_rpc_client =
        //     EthJsonRpcClient::new(ReqwestClient::new(evm_datasource_url.to_string()));

        // let remote_block = evm_rpc_client.get_block_by_number(end_block.into()).await.unwrap();
        // let local_block = provider.block_by_number(end_block).unwrap().unwrap();

        // assert_eq!(remote_block.hash.unwrap().0, local_block.header.hash_slow().0);
        // assert_eq!(remote_block.state_root.0, local_block.state_root.0);
    }
}

fn build_bitfinity_reset_evm_command(identity_name: &str, data_dir: PathBuf, dfx_port: u16, evm_datasource_url: &str) -> BitfinityResetEvmStateCommand {
    let bitfinity_args = BitfinityResetEvmStateArgs {
        evmc_principal: LOCAL_EVM_CANISTER_ID.to_string(),
        ic_identity_file_path: dirs::home_dir().unwrap().join(".config/dfx/identity").join(identity_name).join("identity.pem"),
        evm_network: format!("http://127.0.0.1:{dfx_port}"),
        evm_datasource_url: evm_datasource_url.to_string(),
    };

    let bitfinity_reset_args = BitfinityResetEvmStateCommand {
        datadir: data_dir.into(),
        bitfinity: bitfinity_args,
    };

    bitfinity_reset_args
}