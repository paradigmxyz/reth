//! Command that initializes the reset of remote EVM node using the current node state

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use alloy_rlp::Encodable;
use clap::Parser;
use did::evm_reset_state::EvmResetState;
use did::{AccountInfoMap, RawAccountInfo, H160, H256};
use evm_canister_client::{CanisterClient, EvmCanisterClient, IcAgentClient};
use itertools::Itertools;
use reth_db::cursor::DbCursorRO;
use reth_db::transaction::DbTx;
use reth_db::{init_db, tables, DatabaseEnv};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_node_core::args::BitfinityResetEvmStateArgs;
use reth_node_core::dirs::{DataDirPath, MaybePlatformPath};
use reth_primitives::StorageEntry;
use reth_provider::{BlockNumReader, BlockReader, ProviderFactory};
use tracing::{debug, info, trace, warn};

/// Builder for the `bitfinity reset evm state` command
#[derive(Debug, Parser)]
pub struct BitfinityResetEvmStateCommandBuilder {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    pub datadir: MaybePlatformPath<DataDirPath>,

    /// Bitfinity Related Args
    #[clap(flatten)]
    pub bitfinity: BitfinityResetEvmStateArgs,
}

const MAX_REQUEST_BYTES: usize = 500_000;
const SPLIT_ADD_ACCOUNTS_REQUEST_BYTES: usize = 1_000_000;

impl BitfinityResetEvmStateCommandBuilder {
    /// Build the command
    pub async fn build(self) -> eyre::Result<BitfinityResetEvmStateCommand> {
        let evm_datasource_url = self.bitfinity.evm_datasource_url;
        info!(target: "reth::cli", "Fetching chain spec from: {}", evm_datasource_url);
        let chain = Arc::new(BitfinityEvmClient::fetch_chain_spec(evm_datasource_url).await?);

        let principal = candid::Principal::from_text(self.bitfinity.evmc_principal.as_str())?;
        let evm_client = EvmCanisterClient::new(
            IcAgentClient::with_identity(
                principal,
                self.bitfinity.ic_identity_file_path,
                &self.bitfinity.evm_network,
                None,
            )
            .await?,
        );
        let executor = Arc::new(EvmCanisterResetStateExecutor::new(evm_client));

        let data_dir = self.datadir.unwrap_or_chain_default(chain.chain);
        let db_path = data_dir.db();
        let db = Arc::new(init_db(db_path, Default::default())?);
        let provider_factory = ProviderFactory::new(db.clone(), chain, data_dir.static_files())?;

        Ok(BitfinityResetEvmStateCommand::new(provider_factory, executor))
    }
}

/// Command that initializes the reset of remote EVM node using the current node state
#[derive(Debug)]
pub struct BitfinityResetEvmStateCommand {
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    executor: Arc<dyn ResetStateExecutor>,
}

impl BitfinityResetEvmStateCommand {
    /// Create a new instance of the command
    pub fn new(
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
        executor: Arc<dyn ResetStateExecutor>,
    ) -> Self {
        Self { provider_factory, executor }
    }

    /// Execute the command
    pub async fn execute(&self) -> eyre::Result<()> {
        let mut provider = self.provider_factory.provider()?;
        let last_block_number = provider.last_block_number()?;
        let last_block =
            provider.block_by_number(last_block_number)?.expect("Block should be present");

        info!(target: "reth::cli", "Attempting reset of evm to block {}, state root: {:?}", last_block_number, last_block.state_root);

        // Step 1: Reset the evm, the EVM must be disabled
        {
            self.executor.start().await?;
        }

        // Step 2: Send the state to the EVM
        {
            // TODO: get block number from config. See EPROD-859
            // let GET_BLOCK_FROM_CONFIG = 0;
            // let block_number = provider.last_block_number().unwrap_or_default();
            // let state_provider = provider.state_provider_by_block_number(block_number)?;
            // let res = state_provider.basic_account(...)?;

            let tx_ref = provider.tx_mut();

            // We need to disable the long read transaction safety to avoid the transaction being closed
            tx_ref.disable_long_read_transaction_safety();

            let mut plain_account_cursor = tx_ref.cursor_read::<tables::PlainAccountState>()?;
            let mut contract_storage_cursor = tx_ref.cursor_read::<tables::Bytecodes>()?;

            // We need to iterate through all the accounts and retrieve their storage tries and populate the AccountInfo
            let mut accounts = AccountInfoMap::new();

            info!(target: "reth::cli", "Start recovering storage tries");

            while let Some((ref address, ref account)) = plain_account_cursor.next()? {
                // We need to retrieve the bytecode for the account
                let bytecode = if let Some(bytecode_hash) = account.bytecode_hash {
                    debug!(target: "reth::cli", "Recovering bytecode for account {}", address);
                    contract_storage_cursor
                        .seek_exact(bytecode_hash)?
                        .map(|(_, bytecode)| bytecode.original_bytes().into())
                } else {
                    None
                };

                let mut storage = BTreeMap::new();

                debug!("Recovering storage for account {}", address);

                let mut plain_storage_cursor = tx_ref.cursor_read::<tables::PlainStorageState>()?;
                let mut storage_walker = plain_storage_cursor.walk_range(*address..=*address)?;

                while let Some(result) = storage_walker.next() {
                    let (storage_address, storage_entry) = result?;
                    trace!(
                        "Recovering storage for account {} - found entry: {:?}",
                        address,
                        storage_entry
                    );
                    if storage_address != *address {
                        break;
                    }
                    let StorageEntry { key, value } = storage_entry;

                    let key: reth_primitives::U256 = key.into();
                    storage.insert(key.into(), value.into());
                }

                let account = RawAccountInfo {
                    nonce: account.nonce.into(),
                    balance: account.balance.into(),
                    bytecode,
                    storage: storage.into_iter().collect_vec(),
                };

                debug!(target: "reth::cli", "Account Address: {} Info: {:?}", address, account);

                accounts.data.insert((*address).into(), account);
                debug!(target: "reth::cli", address=%address, "Storage tries recovered");

                if accounts.estimate_byte_size() > MAX_REQUEST_BYTES {
                    let process_accounts = std::mem::replace(&mut accounts, AccountInfoMap::new());
                    split_and_send_add_accout_request(&self.executor, process_accounts).await?;
                }
            }

            if !accounts.data.is_empty() {
                info!(target: "reth::cli", "Processing last batch of {} accounts", accounts.data.len());
                split_and_send_add_accout_request(&self.executor, accounts).await?;
            }

            info!(target: "reth::cli", "Storage tries recovered successfully");
        }

        // Step 3: End of the recovery process. Send block data
        {
            info!(target: "reth::cli", "Preparing to end process by sending block data: {:?}", last_block);
            let mut buff = vec![];
            last_block.encode(&mut buff);

            let did_block = rlp::decode::<did::Block<did::Transaction>>(&buff)?;
            let did_block: did::Block<H256> = did_block.into();
            self.executor.end(did_block).await?;
            info!(target: "reth::cli", "Block data sent successfully");
        }

        info!(target: "reth::cli", "EVM state successfully reset to block {}", last_block_number);

        Ok(())
    }
}

async fn split_and_send_add_accout_request(
    executor: &Arc<dyn ResetStateExecutor>,
    accounts: AccountInfoMap,
) -> eyre::Result<()> {
    for account in split_add_account_request_data(SPLIT_ADD_ACCOUNTS_REQUEST_BYTES, accounts) {
        executor.add_accounts(account).await?;
    }
    Ok(())
}

/// Trait for the reset state executor
pub trait ResetStateExecutor: Send + Debug {
    /// Start the reset state process
    fn start(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>>;

    /// Add accounts to the reset state process
    fn add_accounts(
        &self,
        accounts: AccountInfoMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>>;

    /// End the reset state process
    fn end(
        &self,
        block: did::Block<H256>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>>;
}

/// Executor for the reset state process that uses the EVM canister client
pub struct EvmCanisterResetStateExecutor<C: CanisterClient> {
    client: EvmCanisterClient<C>,
}

impl<C: CanisterClient> Debug for EvmCanisterResetStateExecutor<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmCanisterResetStateExecutor").finish()
    }
}

impl<C: CanisterClient> EvmCanisterResetStateExecutor<C> {
    /// Create a new instance of the executor
    pub fn new(client: EvmCanisterClient<C>) -> Self {
        Self { client }
    }
}

impl<C: CanisterClient + Sync + 'static> ResetStateExecutor for EvmCanisterResetStateExecutor<C> {
    fn start(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            info!(target: "reth::cli", "Send EvmResetState::Start request...");
            client.admin_reset_state(EvmResetState::Start).await??;
            info!(target: "reth::cli", "EvmResetState::Start request sent");
            Ok(())
        })
    }

    fn add_accounts(
        &self,
        accounts: AccountInfoMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            info!(target: "reth::cli", "Send EvmResetState::AddAccounts request with {} accounts...", accounts.data.len());
            client.admin_reset_state(EvmResetState::AddAccounts(accounts)).await??;
            info!(target: "reth::cli", "EvmResetState::AddAccounts request sent");
            Ok(())
        })
    }

    fn end(
        &self,
        block: did::Block<H256>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            info!(target: "reth::cli", "Send EvmResetState::End request...");
            client.admin_reset_state(EvmResetState::End(block)).await??;
            info!(target: "reth::cli", "EvmResetState::End request sent");
            Ok(())
        })
    }
}

/// Split the account request data into multiple requests
fn split_add_account_request_data(
    max_byte_size: usize,
    data: AccountInfoMap,
) -> Vec<AccountInfoMap> {
    if data.estimate_byte_size() <= max_byte_size {
        return vec![data];
    }

    warn!(target: "reth::cli", "Data size exceeds max byte size, splitting data into multiple requests");

    let mut result = vec![];
    let mut current_map = AccountInfoMap::new();

    for (address, account) in data.data {

        for account in split_single_account_data(max_byte_size, account) {
           
            let current_size = current_map.estimate_byte_size();
            let address_size = H160::BYTE_SIZE;
            let account_size = account.estimate_byte_size();
            
            let available_size = max_byte_size.saturating_sub(current_size);
            if address_size + account_size > available_size {
                result.push(std::mem::replace(&mut current_map, AccountInfoMap::new()));
            }
            
            current_map.data.insert(address.clone(), account);
        }
    }

    if !current_map.data.is_empty() {
        result.push(current_map);
    }

    result
}

/// Receive an account and split it into a list of pieces having a size of `max_byte_size`
fn split_single_account_data(
    max_byte_size: usize,
    mut data: RawAccountInfo,
) -> Vec<RawAccountInfo> {
    if data.estimate_byte_size() <= max_byte_size {
        return vec![data];
    }

    warn!(target: "reth::cli", "Single account data size exceeds max byte size, splitting it into multiple requests");

    let mut result = vec![];

    let mut current_account = RawAccountInfo {
        nonce: data.nonce.clone(),
        balance: data.balance.clone(),
        bytecode: None,
        storage: vec![],
    };

    let storage = std::mem::take(&mut data.storage);
    let nonce = data.nonce.clone();
    let balance = data.balance.clone();

    // We push data that contains the bytecode.
    // This works in the case where the bytecode is not larger than the max_byte_size, but this should always be the case
    result.push(data);

    for (key, value) in storage {
        let current_size = current_account.estimate_byte_size();
        let key_size = H256::BYTE_SIZE;
        let value_size = H256::BYTE_SIZE;

        let available_size = max_byte_size.saturating_sub(current_size);
        if key_size + value_size > available_size {
            result.push(std::mem::replace(&mut current_account, RawAccountInfo {
                nonce: nonce.clone(),
                balance: balance.clone(),
                bytecode: None,
                storage: vec![],
            }));
        }

        current_account.storage.push((key, value));
    }

    if !current_account.storage.is_empty() {
        result.push(current_account);
    }



    result
}

#[cfg(test)]
mod test {
    use did::U256;
    use revm_primitives::Address;

    use super::*;

    #[test]
    fn bitfinity_test_split_add_account_request_data() {
        let mut data = AccountInfoMap::new();
        for i in 0u64..100 {
            let address = Address::random().into();
            let account = RawAccountInfo {
                nonce: U256::from(i),
                balance: U256::from(i),
                bytecode: None,
                storage: vec![],
            };
            data.data.insert(address, account);
        }

        let current_size = data.estimate_byte_size();

        {
            let max_size = current_size;
            let result = split_add_account_request_data(max_size, data.clone());
            assert_eq!(result.len(), 1);
            for map in &result {
                assert!(map.estimate_byte_size() <= max_size);
            }
            assert_eq!(result[0], data);
        }

        {
            let max_size = current_size - 1;
            let result = split_add_account_request_data(max_size, data.clone());
            assert_eq!(result.len(), 2);
            for map in &result {
                assert!(map.estimate_byte_size() <= max_size);
            }
            assert_eq!(merge_account_info_maps(result), data);
        }

        {
            let max_size = (current_size / 3) - 1;
            let result = split_add_account_request_data(max_size, data.clone());
            assert_eq!(result.len(), 4);
            for map in &result {
                assert!(map.estimate_byte_size() <= max_size);
            }
            assert_eq!(merge_account_info_maps(result), data);
        }
    }

    fn merge_account_info_maps(maps: Vec<AccountInfoMap>) -> AccountInfoMap {
        let mut result = AccountInfoMap::new();
        for map in maps {
            for (address, account) in map.data {
                result.data.insert(address, account);
            }
        }
        result
    }

    #[test]
    fn bitfinity_split_info_map_should_split_a_single_account_in_multiple_calls() {
        todo!()
    }

    #[test]
    fn bitfinity_test_split_single_account_data() {
        
        let mut account = RawAccountInfo {
            nonce: U256::from(1u64),
            balance: U256::from(1u64),
            bytecode: None,
            storage: vec![],
        };
        
        for i in 0u64..1000 {
            account.storage.push((U256::from(i), U256::from(i)));
        }

        let current_size = account.estimate_byte_size();

        {
            let max_size = current_size;
            let result = split_single_account_data(max_size, account.clone());
            assert_eq!(result.len(), 1);
            for map in &result {
                assert!(map.estimate_byte_size() <= max_size);
            }
            assert_eq!(result[0], account);
        }

        {
            let max_size = current_size - 1;
            let result = split_single_account_data(max_size, account.clone());
            assert_eq!(result.len(), 3);
            for map in &result {
                assert!(map.estimate_byte_size() <= max_size);
            }
            assert_eq!(merge_accounts(result), account);
        }

        {
            let max_size = (current_size / 3) - 1;
            let result = split_single_account_data(max_size, account.clone());
            assert_eq!(result.len(), 5);
            for map in &result {
                assert!(map.estimate_byte_size() <= max_size);
            }
            assert_eq!(merge_accounts(result), account);
        }
    }

    /// Merge accouts into a single account. The nonce, balance and bytecode are taken from the first account
    fn merge_accounts(accounts: Vec<RawAccountInfo>) -> RawAccountInfo {
        let mut result = accounts[0].clone();
        for account in accounts {
            result.storage.extend(account.storage);
        }
        result
    }

}
