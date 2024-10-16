//!
//! Integration tests for the `BitfinityResetEvmStateCommand`.
//! These tests requires a running EVM node or EVM block extractor node at the specified URL.
//!

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use did::{block::BlockResult, AccountInfoMap, H256};
use evm_canister_client::{EvmCanisterClient, IcAgentClient};
use reth::{
    args::BitfinityResetEvmStateArgs,
    commands::bitfinity_reset_evm_state::{
        BitfinityResetEvmStateCommand, EvmCanisterResetStateExecutor, ResetStateExecutor,
    },
};
use reth_db::DatabaseEnv;
use reth_provider::{AccountReader, BlockNumReader, BlockReader, ProviderFactory};
use reth_trie::test_utils::state_root;
use revm_primitives::{keccak256, B256};
use serial_test::serial;
use tracing::*;

use super::utils::*;

/// This test requires a running EVM canister on a local dfx node.
/// When the evm canister WASM will be published, this can be moved to pocket-ic and integrated in the CI pipeline
/// by removing the `manual_` prefix from the test name.
#[tokio::test]
#[serial]
async fn bitfinity_manual_test_should_reset_evm_state() {
    // Arrange
    let _log = init_logs();
    let dfx_port = get_dfx_local_port();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;

    let end_block = 30_000;
    let data_dir = Some(format!("../../target/reth_{end_block}").into());
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, data_dir).await.unwrap();

    let fetch_block_timeout_secs = std::cmp::max(20, end_block / 100);

    // Import block from block explorer
    import_data.bitfinity_args.end_block = Some(end_block);
    import_blocks(import_data.clone(), Duration::from_secs(fetch_block_timeout_secs), true).await;

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

#[tokio::test]
#[serial]
async fn bitfinity_test_reset_should_extract_all_accounts_data() {
    // Arrange
    let _log = init_logs();
    let evm_datasource_url = DEFAULT_EVM_DATASOURCE_URL;

    let end_block = 30_000;
    let data_dir = Some(format!("../../target/reth_{end_block}").into());
    let (_temp_dir, mut import_data) =
        bitfinity_import_config_data(evm_datasource_url, data_dir).await.unwrap();

    let fetch_block_timeout_secs = std::cmp::max(20, end_block / 100);

    // Import block from block explorer
    import_data.bitfinity_args.end_block = Some(end_block);
    import_blocks(import_data.clone(), Duration::from_secs(fetch_block_timeout_secs), true).await;

    let executor = Arc::new(InMemoryResetStateExecutor::default());
    let parallel_requests = 4;
    let reset_state_command = BitfinityResetEvmStateCommand::new(
        import_data.provider_factory.clone(),
        executor.clone(),
        parallel_requests,
    );

    // Act
    {
        info!("Executing bitfinity reset evm state command");
        reset_state_command.execute().await.unwrap();
    }

    // Assert
    {
        // Check that executor was started and has some data
        {
            assert!(executor.is_started());
            assert!(executor.get_accounts_count() > 0);
            assert!(executor.get_block().is_some());
        }

        let provider = import_data.provider_factory.provider().unwrap();
        let last_block = {
            let last_block = provider.last_block_number().unwrap();
            provider.block_by_number(last_block).unwrap().unwrap()
        };

        // Check that block in the extractor is the same as the last block in the provider
        {
            let executor_block = executor.get_block().unwrap();
            assert_eq!(end_block, last_block.number);
            assert_eq!(executor_block.number.0.as_u64(), last_block.number);
            assert_eq!(executor_block.state_root.0 .0, last_block.state_root.0);
        }

        // Check that all accounts in the provider are in the executor
        {
            let executor_accounts = executor.get_accounts();

            let mut accounts_with_code = 0;
            let mut accounts_with_storage_values = 0;
            for (executor_account_address, executor_account) in &executor_accounts.data {
                let account =
                    provider.basic_account(executor_account_address.0 .0.into()).unwrap().unwrap();

                if let Some(bytecode) = &executor_account.bytecode {
                    accounts_with_code += 1;
                    debug!("Account with code: {executor_account_address:?}");
                    trace!("Code: {:?}", bytecode);

                    let code_hash = keccak256(&bytecode.0);
                    debug!("Code hash: {:?}", code_hash);
                    assert_eq!(Some(code_hash), account.bytecode_hash);
                } else {
                    assert!(account.bytecode_hash.is_none());
                }

                if !executor_account.storage.is_empty() {
                    accounts_with_storage_values += 1;
                }
            }

            info!("Executor accounts: {}", executor_accounts.data.len());
            info!("Accounts with code: {accounts_with_code}");
            info!("Accounts with storage values: {accounts_with_storage_values}");
        }

        // Calculate the state root hash from the executor accounts
        {
            let executor_accounts = executor.get_accounts();

            let calculated_root =
                state_root(executor_accounts.data.into_iter().map(|(address, raw_account)| {
                    let account = reth_primitives::Account {
                        nonce: raw_account.nonce.0.as_u64(),
                        balance: raw_account.balance.into(),
                        bytecode_hash: raw_account.bytecode.map(|code| keccak256(&code.0)),
                    };
                    let storage: Vec<(B256, reth_primitives::U256)> = raw_account
                        .storage
                        .into_iter()
                        .map(|(k, v)| {
                            let k: reth_primitives::U256 = k.into();
                            (k.into(), v.into())
                        })
                        .collect();

                    (address.into(), (account, storage))
                }));
            assert_eq!(calculated_root, last_block.state_root);
        }
    }
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
        parallel_requests: 4,
    };

    let principal = candid::Principal::from_text(bitfinity_args.evmc_principal.as_str()).unwrap();

    let evm_client = EvmCanisterClient::new(
        IcAgentClient::with_identity(
            principal,
            bitfinity_args.ic_identity_file_path,
            &bitfinity_args.evm_network,
            None,
        )
        .await
        .unwrap(),
    );

    let executor = Arc::new(EvmCanisterResetStateExecutor::new(evm_client.clone()));

    (
        evm_client,
        BitfinityResetEvmStateCommand::new(
            provider_factory,
            executor,
            bitfinity_args.parallel_requests,
        ),
    )
}

/// In-memory executor for resetting the EVM canister state.
#[derive(Debug, Default)]
struct InMemoryResetStateExecutor {
    started: Mutex<bool>,
    accounts: Mutex<AccountInfoMap>,
    block: Mutex<Option<did::Block<H256>>>,
}

impl InMemoryResetStateExecutor {
    fn is_started(&self) -> bool {
        *self.started.lock().unwrap()
    }

    fn get_accounts(&self) -> AccountInfoMap {
        self.accounts.lock().unwrap().clone()
    }

    fn get_accounts_count(&self) -> usize {
        self.accounts.lock().unwrap().data.len()
    }

    fn get_block(&self) -> Option<did::Block<H256>> {
        self.block.lock().unwrap().clone()
    }
}

impl ResetStateExecutor for InMemoryResetStateExecutor {
    fn start(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        *self.started.lock().unwrap() = true;
        Box::pin(async move { Ok(()) })
    }

    fn add_accounts(
        &self,
        accounts: AccountInfoMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let mut data = self.accounts.lock().unwrap();
        for (address, mut account) in accounts.data {
            // If the account already exists, merge the storage and the bytecode
            if let Some(existing_account) = data.data.get_mut(&address) {
                if let Some(bytecode) = account.bytecode {
                    existing_account.bytecode = Some(bytecode);
                }
                existing_account.storage.append(&mut account.storage);
            } else {
                data.data.insert(address, account);
            }
        }
        Box::pin(async move { Ok(()) })
    }

    fn end(
        &self,
        block: did::Block<H256>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        *self.block.lock().unwrap() = Some(block);
        Box::pin(async move { Ok(()) })
    }
}
