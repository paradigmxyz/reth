//!
//! These are integration tests for the BitfinityResetEvmStateCommand.
//! These tests requires a running EVM node or EVM block extractor node at the specified URL
//! and a running EVM canister on a local dfx node.
//!

use std::{sync::Arc, time::Duration};

use did::block::BlockResult;
use evm_canister_client::{EvmCanisterClient, IcAgentClient};
use reth::{
    args::BitfinityResetEvmStateArgs,
    commands::{
        bitfinity_import::BitfinityImportCommand,
        bitfinity_reset_evm_state::BitfinityResetEvmStateCommand,
    },
};
use reth_db::DatabaseEnv;
use reth_provider::{BlockNumReader, BlockReader, ProviderFactory};

use super::utils::*;

#[tokio::test]
async fn bitfinity_test_should_reset_evm_state() {
    // Arrange
    let dfx_port = get_dfx_local_port();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;
    let (_temp_dir, import_data) = bitfinity_import_config_data(evm_datasource_url).await.unwrap();

    let end_block = 5;

    // Import block from block explorer
    {
        let mut bitfinity_import_args = import_data.bitfinity_args.clone();
        bitfinity_import_args.end_block = Some(end_block);
        let import = BitfinityImportCommand::new(
            None,
            import_data.data_dir.clone(),
            import_data.chain.clone(),
            bitfinity_import_args,
            import_data.provider_factory.clone(),
            import_data.blockchain_db.clone(),
        );
        let (job_executor, _handle) = import.schedule_execution().await.unwrap();
        wait_until_local_block_imported(
            &import_data.provider_factory,
            end_block,
            Duration::from_secs(20),
        )
        .await;
        println!("Stopping job executor");
        job_executor.stop(true).await.unwrap();
        println!("Job executor stopped");
    }

    let (evm_client, reset_state_command) = build_bitfinity_reset_evm_command(
        "alice",
        import_data.provider_factory.clone(),
        dfx_port,
        evm_datasource_url,
    )
    .await;
    let _ = evm_client.admin_disable_evm(true).await.unwrap();

    // Act
    {
        println!("Executing bitfinity reset evm state command");
        reset_state_command.execute().await.unwrap();
    }

    // Assert
    {
        let provider = import_data.provider_factory.provider().unwrap();
        assert_eq!(end_block, provider.last_block_number().unwrap());

        assert_eq!(end_block as usize, evm_client.eth_block_number().await.unwrap());

        let evm_block = match evm_client
            .eth_get_block_by_number(did::BlockNumber::Latest, false)
            .await
            .unwrap()
            .unwrap()
        {
            BlockResult::WithHash(block) => block,
            _ => panic!("Expected full block"),
        };

        assert_eq!(end_block, evm_block.number.0.as_u64());
        let reth_block = provider.block_by_number(end_block).unwrap().unwrap();

        assert_eq!(evm_block.hash.0 .0, reth_block.header.hash_slow().0);
        assert_eq!(evm_block.state_root.0 .0, reth_block.state_root.0);
    }

    let _ = evm_client.admin_disable_evm(false).await.unwrap();
}

async fn build_bitfinity_reset_evm_command(
    identity_name: &str,
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    dfx_port: u16,
    evm_datasource_url: &str,
) -> (EvmCanisterClient<IcAgentClient>, BitfinityResetEvmStateCommand) {
    let bitfinity_args = BitfinityResetEvmStateArgs {
        evmc_principal: LOCAL_EVM_CANISTER_ID.to_string(),
        ic_identity_file_path: dirs::home_dir()
            .unwrap()
            .join(".config/dfx/identity")
            .join(identity_name)
            .join("identity.pem"),
        evm_network: format!("http://127.0.0.1:{dfx_port}"),
        evm_datasource_url: evm_datasource_url.to_string(),
    };

    let principal = candid::Principal::from_text(bitfinity_args.evmc_principal.as_str()).unwrap();

    let evm_client = EvmCanisterClient::new(
        IcAgentClient::with_identity(
            principal,
            bitfinity_args.ic_identity_file_path.clone(),
            &bitfinity_args.evm_network,
            None,
        )
        .await
        .unwrap(),
    );

    (evm_client, BitfinityResetEvmStateCommand::new(provider_factory, bitfinity_args))
}
