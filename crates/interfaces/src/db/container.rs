use crate::db::{Database, DatabaseGAT, DbTx, Error};

/// A container for any DB transaction that will open a new inner transaction when the current
/// one is committed.
// NOTE: This container is needed since `Transaction::commit` takes `mut self`, so methods in
// the pipeline that just take a reference will not be able to commit their transaction and let
// the pipeline continue. Is there a better way to do this?
pub struct DBContainer<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
}

impl<'this, DB> DBContainer<'this, DB>
where
    DB: Database,
{
    /// Create a new container with the given database handle.
    ///
    /// A new inner transaction will be opened.
    pub fn new(db: &'this DB) -> Result<Self, Error> {
        Ok(Self { db, tx: Some(db.tx_mut()?) })
    }

    /// Commit the current inner transaction and open a new one.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [DBContainer::close] was called without following up with a call to [DBContainer::open].
    pub fn commit(&mut self) -> Result<bool, Error> {
        let success =
            self.tx.take().expect("Tried committing a non-existent transaction").commit()?;
        self.tx = Some(self.db.tx_mut()?);
        Ok(success)
    }

    /// Get the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [DBContainer::close] was called without following up with a call to [DBContainer::open].
    pub fn get(&self) -> &<DB as DatabaseGAT<'this>>::TXMut {
        self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
    }

    /// Get a mutable reference to the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [DBContainer::close] was called without following up with a call to [DBContainer::open].
    pub fn get_mut(&mut self) -> &mut <DB as DatabaseGAT<'this>>::TXMut {
        self.tx.as_mut().expect("Tried getting a mutable reference to a non-existent transaction")
    }

    /// Open a new inner transaction.
    pub fn open(&mut self) -> Result<(), Error> {
        self.tx = Some(self.db.tx_mut()?);
        Ok(())
    }

    /// Close the current inner transaction.
    pub fn close(&mut self) {
        self.tx.take();
    }
}

#[cfg(test)]
// This ensures that we can use the GATs in the downstream staged exec pipeline.
mod tests {
    use super::*;
    use crate::db::mock::DatabaseMock;

    #[async_trait::async_trait]
    trait Stage<DB: Database> {
        async fn run(&mut self, db: &mut DBContainer<'_, DB>) -> ();
    }

    struct MyStage<'a, DB>(&'a DB);

    #[async_trait::async_trait]
    impl<'a, DB: Database> Stage<DB> for MyStage<'a, DB> {
        async fn run(&mut self, db: &mut DBContainer<'_, DB>) -> () {
            let _tx = db.commit().unwrap();
        }
    }

    #[test]
    #[should_panic] // no tokio runtime configured
    fn can_spawn() {
        let db = DatabaseMock::default();
        tokio::spawn(async move {
            let mut container = DBContainer::new(&db).unwrap();
            let mut stage = MyStage(&db);
            stage.run(&mut container).await;
        });
    }
}
