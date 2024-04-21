use clap::{Parser, Subcommand};
use reth_db::{
    database::Database,
    static_file::iter_static_files,
    table::Table,
    transaction::{DbTx, DbTxMut},
    TableViewer, Tables,
};
use reth_primitives::{static_file::find_fixed_range, StaticFileSegment};
use reth_provider::ProviderFactory;

/// The arguments for the `reth db clear` command
#[derive(Parser, Debug)]
pub struct Command {
    #[clap(subcommand)]
    subcommand: Subcommands,
}

impl Command {
    /// Execute `db clear` command
    pub fn execute<DB: Database>(self, provider_factory: ProviderFactory<DB>) -> eyre::Result<()> {
        match self.subcommand {
            Subcommands::Mdbx { table } => {
                table.view(&ClearViewer { db: provider_factory.db_ref() })?
            }
            Subcommands::StaticFile { segment } => {
                let static_file_provider = provider_factory.static_file_provider();
                let static_files = iter_static_files(static_file_provider.directory())?;

                if let Some(segment_static_files) = static_files.get(&segment) {
                    for (block_range, _) in segment_static_files {
                        static_file_provider
                            .delete_jar(segment, find_fixed_range(block_range.start()))?;
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
