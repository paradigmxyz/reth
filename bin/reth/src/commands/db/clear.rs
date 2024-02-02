use clap::{Parser, Subcommand};
use reth_db::{
    database::Database,
    snapshot::iter_snapshots,
    table::Table,
    transaction::{DbTx, DbTxMut},
    TableViewer, Tables,
};
use reth_primitives::{snapshot::find_fixed_range, SnapshotSegment};
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
            Subcommands::Snapshot { segment } => {
                let snapshot_provider = provider_factory.snapshot_provider();
                let snapshots = iter_snapshots(snapshot_provider.directory())?;

                if let Some(segment_snapshots) = snapshots.get(&segment) {
                    for (block_range, _) in segment_snapshots {
                        snapshot_provider
                            .delete_jar(segment, find_fixed_range(*block_range.start()))?;
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    Mdbx { table: Tables },
    Snapshot { segment: SnapshotSegment },
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
