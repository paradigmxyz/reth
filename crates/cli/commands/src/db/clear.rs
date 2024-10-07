use clap::{Parser, Subcommand};
use reth_db::{static_file::iter_static_files, TableViewer, Tables};
use reth_db_api::{
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::{ProviderFactory, StaticFileProviderFactory};
use reth_static_file_types::StaticFileSegment;

/// The arguments for the `reth db clear` command
#[derive(Parser, Debug)]
pub struct Command {
    #[clap(subcommand)]
    subcommand: Subcommands,
}

impl Command {
    /// Execute `db clear` command
    pub fn execute<N: NodeTypesWithDB>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        match self.subcommand {
            Subcommands::Mdbx { table } => {
                table.view(&ClearViewer { db: provider_factory.db_ref() })?
            }
            Subcommands::StaticFile { segment } => {
                let static_file_provider = provider_factory.static_file_provider();
                let static_files = iter_static_files(static_file_provider.directory())?;

                if let Some(segment_static_files) = static_files.get(&segment) {
                    for (block_range, _) in segment_static_files {
                        static_file_provider.delete_jar(segment, block_range.start())?;
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    /// Deletes all database table entries
    Mdbx { table: Tables },
    /// Deletes all static file segment entries
    StaticFile { segment: StaticFileSegment },
}

struct ClearViewer<'a, DB: Database> {
    db: &'a DB,
}

impl<DB: Database> TableViewer<()> for ClearViewer<'_, DB> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let tx = self.db.tx_mut()?;
        tx.clear::<T>()?;
        tx.commit()?;
        Ok(())
    }
}
