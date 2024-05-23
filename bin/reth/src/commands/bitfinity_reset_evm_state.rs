//! Command that initializes the reset of remote EVM node using the current node state

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use alloy_rlp::Encodable;
use clap::Parser;
use did::evm_reset_state::EvmResetState;
use did::{AccountInfoMap, RawAccountInfo, H160, H256, U256};
use evm_canister_client::{CanisterClient, EvmCanisterClient, IcAgentClient};
use itertools::Itertools;
use reth_db::cursor::DbCursorRO;
use reth_db::transaction::DbTx;
use reth_db::{init_db, tables, DatabaseEnv};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_node_core::args::BitfinityResetEvmStateArgs;
use reth_node_core::dirs::{DataDirPath, MaybePlatformPath};
use reth_primitives::{StorageEntry, B256};
use reth_provider::{BlockNumReader, BlockReader, ProviderFactory};
use tracing::{debug, info, trace};

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
    pub fn new(provider_factory: ProviderFactory<Arc<DatabaseEnv>>, executor: Arc<dyn ResetStateExecutor>) -> Self {
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
            let mut plain_storage_cursor = tx_ref.cursor_read::<tables::PlainStorageState>()?;

            // We need to iterate through all the accounts and retrieve their storage tries and populate the AccountInfo
            let mut accounts = AccountInfoMap::new();

            info!(target: "reth::cli", "Start recovering storage tries");

            let mut batch_size = 0;
            let batch_limit = 500;



            while let Some((ref address, ref account)) = plain_account_cursor.next()? {
                // We need to retrieve the bytecode for the account
                let bytecode = if let Some(bytecode_hash) = account.bytecode_hash {
                    debug!(target: "reth::cli", "Recovering bytecode for account {}", address);
                    contract_storage_cursor.seek_exact(bytecode_hash)?.map(|(_, bytecode)| bytecode.original_bytes().into())
                } else {
                    None
                };

                let mut storage = BTreeMap::new();

                fn b256_to_u256(num: B256) -> reth_primitives::U256 {
                    reth_primitives::U256::from_be_bytes(num.0)
                }

                debug!("Recovering storage for account {}", address);

                let mut storage_walker = plain_storage_cursor.walk_range(*address..=*address)?;
                while let Some(result) = storage_walker.next() {
                    let (key, entry) = result?;
                    trace!("Recovering storage for account {} - found entry: {:?}", address, entry);
                    if key == *address {
                        continue;
                    }
                    let StorageEntry { key, value } = entry;

                    let REMOVE_ME = 0;
                    info!(target: "reth::cli", "Recovering storage for account {} - found entry: {:?}", address, entry);

                    storage.insert(
                        b256_to_u256(key).into(),
                        did::U256::from_little_endian(&value.as_le_bytes_trimmed()),
                    );
                }

                let account = RawAccountInfo {
                    nonce: account.nonce.into(),
                    balance: account.balance.into(),
                    bytecode,
                    storage: storage.into_iter().collect_vec(),
                };

                debug!(target: "reth::cli", "Account Address: {} Info: {:?}", address, account);

                accounts.insert((*address).into(), account);
                debug!(target: "reth::cli", address=%address, "Storage tries recovered");

                batch_size += 1;
                if batch_size == batch_limit || estimate_size(&accounts) > MAX_REQUEST_BYTES {
                    let process_accounts = std::mem::replace(&mut accounts, AccountInfoMap::new());
                    self.executor.add_accounts(process_accounts).await?;
                    batch_size = 0;
                }
            }

            if !accounts.is_empty() {
                info!(target: "reth::cli", "Processing last batch of {} accounts", accounts.len());
                self.executor.add_accounts(accounts).await?;
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

fn estimate_size(map: &AccountInfoMap) -> usize {
    let mut total_size = 0;

    for (_address, account) in map {
        // key
        let address_size = H160::BYTE_SIZE;
        
        // value
        let nonce_size = U256::BYTE_SIZE;
        let balance_size = U256::BYTE_SIZE;
        let bytecode_size = account.bytecode.as_ref().map(|b| b.0.len()).unwrap_or(0);
        let storage_size = U256::BYTE_SIZE * 2 * account.storage.len();
        
        total_size += address_size + nonce_size + balance_size + bytecode_size + storage_size;

    }
    
    total_size
}

/// Trait for the reset state executor
pub trait ResetStateExecutor: Send + Debug {

    /// Start the reset state process
    fn start(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>>;

    /// Add accounts to the reset state process
    fn add_accounts(&self, accounts: AccountInfoMap) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>>;

    /// End the reset state process
    fn end(&self, block: did::Block<H256>) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>>;
    
}

/// Executor for the reset state process that uses the EVM canister client
pub struct EvmCanisterResetStateExecutor<C: CanisterClient> {
    client: EvmCanisterClient<C>,
}

impl <C: CanisterClient> Debug for EvmCanisterResetStateExecutor<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmCanisterResetStateExecutor").finish()
    }
}

impl <C: CanisterClient> EvmCanisterResetStateExecutor<C> {
    /// Create a new instance of the executor
    pub fn new(client: EvmCanisterClient<C>) -> Self {
        Self { client }
    }
}

impl <C: CanisterClient + Sync + 'static> ResetStateExecutor for EvmCanisterResetStateExecutor<C> {

    fn start(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let client= self.client.clone();
        Box::pin(async move {
            info!(target: "reth::cli", "Send EvmResetState::Start request...");
            let res = client.admin_reset_state(EvmResetState::Start).await??;
            info!(target: "reth::cli", "EvmResetState::Start request sent");
            Ok(res)
        })
    }

    fn add_accounts(&self, accounts: AccountInfoMap) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let client= self.client.clone();
        Box::pin(async move {
            info!(target: "reth::cli", "Send EvmResetState::AddAccounts request with {} accounts...", accounts.len());
            let res = client.admin_reset_state(EvmResetState::AddAccounts(accounts.clone())).await??;
            info!(target: "reth::cli", "EvmResetState::AddAccounts request sent");
            Ok(res)
        })
    }

    fn end(&self, block: did::Block<H256>) -> std::pin::Pin<Box<dyn std::future::Future<Output = eyre::Result<()>> + Send>> {
        let client= self.client.clone();
        Box::pin(async move {
            info!(target: "reth::cli", "Send EvmResetState::End request...");
            let res = client.admin_reset_state(EvmResetState::End(block)).await??;
            info!(target: "reth::cli", "EvmResetState::End request sent");
            Ok(res)
        })
    }

}