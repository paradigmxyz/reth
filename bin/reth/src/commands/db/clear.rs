use clap::Parser;
use reth_db::{
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
    TableViewer, Tables,
};

/// The arguments for the `reth db clear` command
#[derive(Parser, Debug)]
pub struct Command {
    /// Table name
    pub table: Tables,
}

impl Command {
    /// Execute `db clear` command
    pub fn execute<DB: Database>(self, db: &DB) -> eyre::Result<()> {
        self.table.view(&ClearViewer { db })?;

        Ok(())
    }
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
