//! Command that initializes the reset of remote EVM node using the current node state

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use alloy_rlp::Encodable;
use clap::Parser;
use did::evm_reset_state::EvmResetState;
use did::{AccountInfoMap, RawAccountInfo, H256};
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
use tracing::{debug, info};

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

impl BitfinityResetEvmStateCommandBuilder {

    /// Build the command
    pub async fn build(self) -> eyre::Result<BitfinityResetEvmStateCommand> {
        let evm_datasource_url = self.bitfinity.evm_datasource_url.clone();
        info!(target: "reth::cli", "Fetching chain spec from: {}", evm_datasource_url);
        let chain = Arc::new(BitfinityEvmClient::fetch_chain_spec(evm_datasource_url).await?);

        let data_dir = self.datadir.unwrap_or_chain_default(chain.chain);
        let db_path = data_dir.db();
        let db = Arc::new(init_db(db_path, Default::default())?);
        let provider_factory = ProviderFactory::new(db.clone(), chain, data_dir.static_files())?;

        Ok(BitfinityResetEvmStateCommand::new(provider_factory, self.bitfinity))
    }
}

/// Command that initializes the reset of remote EVM node using the current node state
#[derive(Debug)]
pub struct BitfinityResetEvmStateCommand {
    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    bitfinity: BitfinityResetEvmStateArgs,
}

impl BitfinityResetEvmStateCommand {

    /// Create a new instance of the command
    pub fn new(provider_factory: ProviderFactory<Arc<DatabaseEnv>>, bitfinity: BitfinityResetEvmStateArgs) -> Self {
        Self { provider_factory, bitfinity }
    }

    /// Execute the command
    pub async fn execute(&self) -> eyre::Result<()> {
        let principal = candid::Principal::from_text(self.bitfinity.evmc_principal.as_str())?;

        let evm_client = EvmCanisterClient::new(
            IcAgentClient::with_identity(
                principal,
                self.bitfinity.ic_identity_file_path.clone(),
                &self.bitfinity.evm_network,
                None,
            )
            .await?,
        );

        let provider = self.provider_factory.provider()?;
        let last_block_number = provider.last_block_number()?;
        let last_block =
            provider.block_by_number(last_block_number)?.expect("Block should be present");

        info!(target: "reth::cli", "Attempting reset of evm to block {}, state root: {:?}", last_block_number, last_block.state_root);

        // Step 1: Reset the evm, the EVM must be disabled
        {
            info!(target: "reth::cli", "Send EvmResetState::Start request...");
            evm_client.admin_reset_state(EvmResetState::Start).await??;
            info!(target: "reth::cli", "EvmResetState::Start request sent");
        }

        // Step 2: Send the state to the EVM
        {
            // TODO: get block number from config. See EPROD-859
            // let GET_BLOCK_FROM_CONFIG = 0;
            // let block_number = provider.last_block_number().unwrap_or_default();
            // let state_provider = provider.state_provider_by_block_number(block_number)?;
            // let res = state_provider.basic_account(...)?;

            let tx_ref = provider.tx_ref();

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
                    contract_storage_cursor.seek_exact(bytecode_hash)?.map(|(_, bytecode)| bytecode)
                } else {
                    None
                };

                let mut storage = BTreeMap::new();

                fn b256_to_u256(num: B256) -> reth_primitives::U256 {
                    reth_primitives::U256::from_be_bytes(num.0)
                }

                while let Some((_, entry)) = plain_storage_cursor.seek_exact(*address)? {
                    debug!("Recovering storage for account {}", address);
                    let StorageEntry { key, value } = entry;
                    storage.insert(
                        b256_to_u256(key).into(),
                        did::U256::from_little_endian(&value.as_le_bytes_trimmed()),
                    );
                }

                let account = RawAccountInfo {
                    nonce: account.nonce.into(),
                    balance: account.balance.into(),
                    bytecode: bytecode.map(|bytecode| bytecode.bytes().into()),
                    storage: storage.into_iter().collect_vec(),
                };

                debug!(target: "reth::cli", "Account Address: {} Info: {:?}", address, account);

                accounts.insert((*address).into(), account);
                debug!(target: "reth::cli", address=%address, "Storage tries recovered");

                debug!(target: "reth::cli", batch_size=%batch_size, "Processing batch of accounts");
                batch_size += 1;

                if batch_size == batch_limit {
                    let process_accounts = std::mem::replace(&mut accounts, AccountInfoMap::new());
                    info!(target: "reth::cli", "Processing batch of {} accounts", batch_size);
                    Self::process_account_info(&evm_client, process_accounts).await?;
                    batch_size = 0;
                }
            }

            if !accounts.is_empty() {
                info!(target: "reth::cli", "Processing last batch of {} accounts", accounts.len());
                Self::process_account_info(&evm_client, accounts).await?;
            }

            info!(target: "reth::cli", "Storage tries recovered successfully");
        }

        // Step 3: End of the recovery process. Send block data
        {
            info!(target: "reth::cli", "Preparing to end process by sending block data...");
            let mut buff = vec![];
            last_block.encode(&mut buff);

            let did_block = rlp::decode::<did::Block<did::Transaction>>(&buff)?;
            let did_block: did::Block<H256> = did_block.into();
            evm_client.admin_reset_state(EvmResetState::End(did_block)).await??;
            info!(target: "reth::cli", "Block data sent successfully");
        }

        info!(target: "reth::cli", "EVM state successfully reset to block {}", last_block_number);

        Ok(())
    }

    async fn process_account_info(
        client: &EvmCanisterClient<impl CanisterClient>,
        accounts: AccountInfoMap,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Processing account info");
        client.admin_reset_state(EvmResetState::AddAccounts(accounts)).await??;
        Ok(())
    }
}
