use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use did::{AccountInfoMap, RawAccountInfo};
use ethereum_json_rpc_client::reqwest::ReqwestClient;
use evm_canister_client::{CanisterClient, EvmCanisterClient, IcAgentClient};
use itertools::Itertools;
use reth_config::Config;
use reth_db::cursor::DbCursorRO;
use reth_db::transaction::DbTx;
use reth_db::{init_db, tables};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_node_core::args::{BitfinityExportToEvmArgs, BitfinityImportArgs, DatabaseArgs};
use reth_node_core::dirs::{DataDirPath, MaybePlatformPath};
use reth_node_core::init::init_genesis;
use reth_primitives::StorageEntry;
use reth_provider::{BlockNumReader, ProviderFactory};
use tracing::{debug, info};

use crate::commands::import::ImportCommand;

/// `reth recover` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// Bitfinity Related Args
    #[clap(flatten)]
    bitfinity: BitfinityExportToEvmArgs,
}

impl Command {
    /// Execute `storage-tries` recovery command
    pub async fn execute(self) -> eyre::Result<()> {

        let principal = candid::Principal::from_text(self.bitfinity.evmc_principal.as_str())?;

        let evm_client = EvmCanisterClient::new(
            IcAgentClient::with_identity(
                principal,
                self.bitfinity.ic_identity_file_path.clone(),
                &self.bitfinity.evm_url,
                None,
            )
            .await?,
        );

        let chain = Arc::new(
            BitfinityEvmClient::fetch_chain_spec(self.bitfinity.evm_url.clone().to_owned()).await?,
        );

        let data_dir = self.datadir.unwrap_or_chain_default(chain.chain);

        let db_path = data_dir.db();
        fs::create_dir_all(&db_path)?;

        let db = Arc::new(init_db(db_path, Default::default())?);

        let provider_factory =
            ProviderFactory::new(db.clone(), chain.clone(), data_dir.static_files())?;

        // Disable evm execution

        {
            evm_client.admin_disable_evm(true).await??;
            evm_client.admin_disable_process_pending_transactions(true).await??;
        }

        //
        {
            // Reset the evm
            info!(target: "reth::cli", "Resetting evm");
            evm_client.reset_state().await??;
            info!(target: "reth::cli", "Evm reset");
        }

        let mut provider = provider_factory.provider()?;

        let tx_mut = provider.tx_mut();
        
        let mut plain_account_cursor = tx_mut.cursor_read::<tables::PlainAccountState>()?;
        let mut contract_storage_cursor = tx_mut.cursor_read::<tables::Bytecodes>()?;
        let mut plain_storage_cursor = tx_mut.cursor_read::<tables::PlainStorageState>()?;

        let mut entry = plain_account_cursor.first()?;

        // We need to iterate through all the accounts and retrieve their storage tries and populate the AccountInfo
        let mut accounts = AccountInfoMap::new();

        info!(target: "reth::cli", "Recovering storage tries");

        let mut batch_size = 0;
        let batch_limit = 500;

        while let Some((ref address, ref account)) = entry {
            let bytecode = contract_storage_cursor
                .seek_exact(account.bytecode_hash.unwrap_or_default())?
                .map(|bytecode| bytecode.1.bytecode.clone());

            let mut storage = BTreeMap::new();

            while let Some((_, entry)) = plain_storage_cursor.seek_exact(*address)? {
                info!("Recovering storage for account {}", address);
                let StorageEntry { key, value } = entry;
                storage.insert(
                    did::H256::from_slice(&key.0),
                    did::U256::from_little_endian(&value.as_le_bytes_trimmed()),
                );
            }

            let account = RawAccountInfo {
                nonce: account.nonce.into(),
                balance: account.balance.into(),
                bytecode: bytecode.map(Into::into),
                storage: storage.into_iter().collect_vec(),
            };

            info!(target: "reth::cli", "Account Address: {} Info: {:?}", address, account);

            info!(target: "reth::cli",address=%address, "Recovering storage tries");

            accounts.insert((*address).into(), account);
            info!(target: "reth::cli", address=%address, "Storage tries recovered");

            info!(target: "reth::cli", batch_size=%batch_size, "Processing batch of accounts");
            batch_size += 1;

            if batch_size == batch_limit {
                info!(target: "reth::cli", "Processing batch of accounts");
                Self::process_account_info(&evm_client, &mut accounts).await?;
                accounts.clear();
                batch_size = 0;
            }

            entry = plain_account_cursor.next()?;
        }

        info!(target: "reth::cli", "Processing last batch of accounts");

        if !accounts.is_empty() {
            Self::process_account_info(&evm_client, &mut accounts).await?;
        }

        Ok(())
    }

    async fn process_account_info(
        client: &EvmCanisterClient<impl CanisterClient>,
        accounts: &mut AccountInfoMap,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Processing account info");

        client.update_state(accounts.clone()).await??;

        Ok(())
    }
}