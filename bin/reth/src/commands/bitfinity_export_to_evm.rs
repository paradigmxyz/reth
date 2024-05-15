//! Command that initializes the reset of remote EVM node using the current node state
//! 
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use alloy_rlp::Encodable;
use clap::Parser;
use did::evm_reset_state::EvmResetState;
use did::{AccountInfoMap, RawAccountInfo, H256};
use evm_canister_client::{CanisterClient, EvmCanisterClient, IcAgentClient};
use itertools::Itertools;
use reth_db::cursor::DbCursorRO;
use reth_db::transaction::DbTx;
use reth_db::{init_db, tables};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_node_core::args::BitfinityExportToEvmArgs;
use reth_node_core::dirs::{DataDirPath, MaybePlatformPath};
use reth_primitives::{StorageEntry, B256};
use reth_provider::{BlockNumReader, BlockReader, ProviderFactory};
use tracing::{debug, info};

/// Reset the EVM state to a specific block number.
#[derive(Debug, Parser)]
pub struct BitfinityResetEvmStateCommand {
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

impl BitfinityResetEvmStateCommand {
    /// Execute the command
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

        println!("Fetching chain spec" );
        let FIX_ME = false;
        let URL = "https://orca-app-5yyst.ondigitalocean.app";
        let chain = Arc::new(

            BitfinityEvmClient::fetch_chain_spec(URL.to_owned()).await?,
        );
        println!("Chain: {:?}", chain.chain);
        let data_dir = self.datadir.unwrap_or_chain_default(chain.chain);

        let db_path = data_dir.db();
        fs::create_dir_all(&db_path)?;

        let db = Arc::new(init_db(db_path, Default::default())?);

        let provider_factory =
            ProviderFactory::new(db.clone(), chain.clone(), data_dir.static_files())?;

        // Disable evm execution
        {
            println!("admin_disable_evm");
            evm_client.admin_disable_evm(true).await??;
            println!("admin_disable_evm done");
        }

        // Reset the evm
        {
            info!(target: "reth::cli", "Resetting evm");
            evm_client.admin_reset_state(EvmResetState::Start).await??;
            info!(target: "reth::cli", "Evm reset");
        }

        let provider = provider_factory.provider()?;

        // TODO: get block number from config
        // let GET_BLOCK_FROM_CONFIG = 0;
        // let block_number = provider.last_block_number().unwrap_or_default();
        // let state_provider = provider.state_provider_by_block_number(block_number)?;
        // let res = state_provider.basic_account(...)?;

        let last_block_number = provider.last_block_number()?;
        let last_block = provider.block_by_number(last_block_number)?.expect("Block should be present");

        info!(target: "reth::cli", "Resetting evm to block {}, state root: {:?}", last_block_number, last_block.state_root);

        let tx_ref = provider.tx_ref();
        
        let mut plain_account_cursor = tx_ref.cursor_read::<tables::PlainAccountState>()?;
        let mut contract_storage_cursor = tx_ref.cursor_read::<tables::Bytecodes>()?;
        let mut plain_storage_cursor = tx_ref.cursor_read::<tables::PlainStorageState>()?;

        // We need to iterate through all the accounts and retrieve their storage tries and populate the AccountInfo
        let mut accounts = AccountInfoMap::new();

        info!(target: "reth::cli", "Recovering storage tries");

        let mut batch_size = 0;
        let batch_limit = 500;

        while let Some((ref address, ref account)) = plain_account_cursor.next()? {

            // We need to retrieve the bytecode for the account
            let bytecode = if let Some(bytecode_hash) = account.bytecode_hash {
                info!(target: "reth::cli", "Recovering bytecode for account {}", address);
                contract_storage_cursor
                    .seek_exact(bytecode_hash)?
                    .map(|(_, bytecode)| bytecode)
            } else {
                None
            };

            let mut storage = BTreeMap::new();

            fn b256_to_u256(num: B256) -> reth_primitives::U256 {
                reth_primitives::U256::from_be_bytes(num.0)
            }

            while let Some((_, entry)) = plain_storage_cursor.seek_exact(*address)? {
                info!("Recovering storage for account {}", address);
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

            info!(target: "reth::cli",address=%address, "Recovering storage tries");

            accounts.insert((*address).into(), account);
            info!(target: "reth::cli", address=%address, "Storage tries recovered");

            info!(target: "reth::cli", batch_size=%batch_size, "Processing batch of accounts");
            batch_size += 1;

            if batch_size == batch_limit {
                let process_accounts = std::mem::replace(&mut accounts, AccountInfoMap::new());
                info!(target: "reth::cli", "Processing batch of accounts");
                Self::process_account_info(&evm_client, process_accounts).await?;
                batch_size = 0;
            }

        }

        info!(target: "reth::cli", "Processing last batch of accounts");

        if !accounts.is_empty() {
            Self::process_account_info(&evm_client, accounts).await?;
        }

        info!(target: "reth::cli", "Storage tries recovered");

        // End of the recovery process
        {
            info!(target: "reth::cli", "End of recovery process");

            let mut buff = vec![];
            last_block.encode(&mut buff);

            let did_block = rlp::decode::<did::Block<did::Transaction>>(&buff)?;
            let did_block: did::Block<H256> = did_block.into();

            // let block_json = serde_json::to_value(last_block)?;
            // let did_block: did::Block<H256> = serde_json::from_value(block_json)?;
            evm_client.admin_reset_state(EvmResetState::End(did_block)).await??;
        }

        // Enable evm execution
        {
            // Should I do it automatically?
            let SHOULD_I_REALLY_DO_IT_AUTOMATICALLY = false;
            evm_client.admin_disable_evm(false).await??;
        }

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