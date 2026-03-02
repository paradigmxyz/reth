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
    /// Enable or disable v2 storage layout
    ///
    /// When enabled, uses static files for receipts/senders/changesets and RocksDB for
    /// history indices and transaction hashes. When disabled, uses v1/legacy layout (everything in
    /// MDBX).
    V2 {
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

        let mut settings @ StorageSettings { storage_v2: _ } =
            settings.unwrap_or_else(StorageSettings::v1);

        // Update the setting based on the key
        match cmd {
            SetCommand::V2 { value } => {
                if settings.storage_v2 == value {
                    println!("storage_v2 is already set to {}", value);
                    return Ok(());
                }
                settings.storage_v2 = value;
                println!("Set storage_v2 = {}", value);
            }
        }

        // Write updated settings
        provider_rw.write_storage_settings(settings)?;
        provider_rw.commit()?;

        println!("Storage settings updated successfully.");

        Ok(())
    }
}
