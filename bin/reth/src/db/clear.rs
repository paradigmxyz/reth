use clap::Parser;

use reth_db::{database::Database, table::Table, transaction::DbTxMut, TableViewer, Tables};

/// The arguments for the `reth db clear` command
#[derive(Parser, Debug)]
pub struct Command {
    /// Table name
    #[arg()]
    pub table: Tables,
}

impl Command {
    /// Execute `db clear` command
    pub fn execute<DB: Database>(self, db: &DB) -> eyre::Result<()> {
        self.table.view(&ClearViewer { db, args: &self })?;

        Ok(())
    }
}

struct ClearViewer<'a, DB: Database> {
    db: &'a DB,
    args: &'a Command,
}

impl<DB: Database> TableViewer<()> for ClearViewer<'_, DB> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let tx = self.db.tx_mut()?;
        tx.clear::<T>()?;

        Ok(())
    }
}
