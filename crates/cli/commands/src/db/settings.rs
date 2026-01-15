//! `reth db settings` command for managing storage settings

use clap::{ArgAction, Parser, Subcommand};
use reth_db_common::DbTool;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, MetadataProvider,
    MetadataWriter, StorageSettings,
};

use crate::common::AccessRights;

/// `reth db settings` subcommand
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Returns database access rights required for the command.
    pub fn access_rights(&self) -> AccessRights {
        match self.command {
            Subcommands::Get => AccessRights::RO,
            Subcommands::Set(_) => AccessRights::RW,
        }
    }
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum Subcommands {
    /// Get current storage settings from database
    Get,
    /// Set storage settings in database
    #[clap(subcommand)]
    Set(SetCommand),
}

/// Set storage settings
#[derive(Debug, Clone, Copy, Subcommand)]
#[clap(rename_all = "snake_case")]
pub enum SetCommand {
    /// Store receipts in static files instead of the database
    Receipts {
        #[clap(action(ArgAction::Set))]
        value: bool,
    },
    /// Store transaction senders in static files instead of the database
    TransactionSenders {
        #[clap(action(ArgAction::Set))]
        value: bool,
    },
    /// Store account changesets in static files instead of the database
    AccountChangesets {
        #[clap(action(ArgAction::Set))]
        value: bool,
    },
    /// Store storage changesets in static files instead of the database
    StorageChangesets {
        #[clap(action(ArgAction::Set))]
        value: bool,
    },
}

impl Command {
    /// Execute the command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.command {
            Subcommands::Get => self.get(tool),
            Subcommands::Set(cmd) => self.set(cmd, tool),
        }
    }

    fn get<N: ProviderNodeTypes>(&self, tool: &DbTool<N>) -> eyre::Result<()> {
        // Read storage settings
        let provider = tool.provider_factory.provider()?;
        let storage_settings = provider.storage_settings()?;

        // Display settings
        match storage_settings {
            Some(settings) => {
                println!("Current storage settings:");
                println!("{settings:#?}");
            }
            None => {
                println!("No storage settings found.");
            }
        }

        Ok(())
    }

    fn set<N: ProviderNodeTypes>(&self, cmd: SetCommand, tool: &DbTool<N>) -> eyre::Result<()> {
        // Read storage settings
        let provider_rw = tool.provider_factory.database_provider_rw()?;
        // Destruct settings struct to not miss adding support for new fields
        let settings = provider_rw.storage_settings()?;
        if settings.is_none() {
            println!("No storage settings found, creating new settings.");
        }

        let mut settings @ StorageSettings {
            receipts_in_static_files: _,
            transaction_senders_in_static_files: _,
            storages_history_in_rocksdb: _,
            transaction_hash_numbers_in_rocksdb: _,
            account_history_in_rocksdb: _,
            account_changesets_in_static_files: _,
            storage_changesets_in_static_files: _,
        } = settings.unwrap_or_else(StorageSettings::legacy);

        // Update the setting based on the key
        match cmd {
            SetCommand::Receipts { value } => {
                if settings.receipts_in_static_files == value {
                    println!("receipts_in_static_files is already set to {}", value);
                    return Ok(());
                }
                settings.receipts_in_static_files = value;
                println!("Set receipts_in_static_files = {}", value);
            }
            SetCommand::TransactionSenders { value } => {
                if settings.transaction_senders_in_static_files == value {
                    println!("transaction_senders_in_static_files is already set to {}", value);
                    return Ok(());
                }
                settings.transaction_senders_in_static_files = value;
                println!("Set transaction_senders_in_static_files = {}", value);
            }
            SetCommand::AccountChangesets { value } => {
                if settings.account_changesets_in_static_files == value {
                    println!("account_changesets_in_static_files is already set to {}", value);
                    return Ok(());
                }
                settings.account_changesets_in_static_files = value;
                println!("Set account_changesets_in_static_files = {}", value);
            }
            SetCommand::StorageChangesets { value } => {
                if settings.storage_changesets_in_static_files == value {
                    println!("storage_changesets_in_static_files is already set to {}", value);
                    return Ok(());
                }
                settings.storage_changesets_in_static_files = value;
                println!("Set storage_changesets_in_static_files = {}", value);
            }
        }

        // Write updated settings
        provider_rw.write_storage_settings(settings)?;
        provider_rw.commit()?;

        println!("Storage settings updated successfully.");

        Ok(())
    }
}
