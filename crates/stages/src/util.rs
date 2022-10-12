pub(crate) mod opt {
    use tokio::sync::mpsc::{error::SendError, Sender};

    /// Get an [Option] with the maximum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn max<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::max(v, b)))
    }

    /// Get an [Option] with the minimum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn min<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::min(v, b)))
    }

    /// The producing side of a [tokio::mpsc] channel that may or may not be set.
    #[derive(Default, Clone)]
    pub(crate) struct MaybeSender<T> {
        inner: Option<Sender<T>>,
    }

    impl<T> MaybeSender<T> {
        /// Create a new [MaybeSender]
        pub(crate) fn new(sender: Option<Sender<T>>) -> Self {
            Self { inner: sender }
        }

        /// Send a value over the channel if an internal sender has been set.
        pub(crate) async fn send(&self, value: T) -> Result<(), SendError<T>> {
            if let Some(rx) = &self.inner {
                rx.send(value).await
            } else {
                Ok(())
            }
        }

        /// Set or unset the internal sender.
        pub(crate) fn set(&mut self, sender: Option<Sender<T>>) {
            self.inner = sender;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opt_max() {
            assert_eq!(max(None, 5), Some(5));
            assert_eq!(max(Some(1), 5), Some(5));
            assert_eq!(max(Some(10), 5), Some(10));
        }

        #[test]
        fn opt_min() {
            assert_eq!(min(None, 5), Some(5));
            assert_eq!(min(Some(1), 5), Some(1));
            assert_eq!(min(Some(10), 5), Some(5));
        }
    }
}

pub(crate) mod db {
    use reth_db::{
        kv::{tx::Tx, Env, KVError},
        mdbx,
    };

    /// A container for a MDBX transaction that will open a new inner transaction when the current
    /// one is committed.
    // NOTE: This container is needed since `Transaction::commit` takes `mut self`, so methods in
    // the pipeline that just take a reference will not be able to commit their transaction and let
    // the pipeline continue. Is there a better way to do this?
    pub(crate) struct TxContainer<'db, 'tx, E>
    where
        'db: 'tx,
        E: mdbx::EnvironmentKind,
    {
        /// A handle to the MDBX database.
        pub(crate) db: &'db Env<E>,
        tx: Option<Tx<'tx, mdbx::RW, E>>,
    }

    impl<'db, 'tx, E> TxContainer<'db, 'tx, E>
    where
        'db: 'tx,
        E: mdbx::EnvironmentKind,
    {
        /// Create a new container with the given database handle.
        ///
        /// A new inner transaction will be opened.
        pub(crate) fn new(db: &'db Env<E>) -> Result<Self, KVError> {
            Ok(Self { db, tx: Some(Tx::new(db.begin_rw_txn()?)) })
        }

        /// Commit the current inner transaction and open a new one.
        ///
        /// # Panics
        ///
        /// Panics if an inner transaction does not exist. This should never be the case unless
        /// [TxContainer::close] was called without following up with a call to [TxContainer::open].
        pub(crate) fn commit(&mut self) -> Result<bool, KVError> {
            let success =
                self.tx.take().expect("Tried committing a non-existent transaction").commit()?;
            self.tx = Some(Tx::new(self.db.begin_rw_txn()?));
            Ok(success)
        }

        /// Get the inner transaction.
        ///
        /// # Panics
        ///
        /// Panics if an inner transaction does not exist. This should never be the case unless
        /// [TxContainer::close] was called without following up with a call to [TxContainer::open].
        pub(crate) fn get(&self) -> &Tx<'tx, mdbx::RW, E> {
            self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
        }

        /// Get a mutable reference to the inner transaction.
        ///
        /// # Panics
        ///
        /// Panics if an inner transaction does not exist. This should never be the case unless
        /// [TxContainer::close] was called without following up with a call to [TxContainer::open].
        pub(crate) fn get_mut(&mut self) -> &mut Tx<'tx, mdbx::RW, E> {
            self.tx
                .as_mut()
                .expect("Tried getting a mutable reference to a non-existent transaction")
        }

        /// Open a new inner transaction.
        pub(crate) fn open(&mut self) -> Result<(), KVError> {
            self.tx = Some(Tx::new(self.db.begin_rw_txn()?));
            Ok(())
        }

        /// Close the current inner transaction.
        pub(crate) fn close(&mut self) {
            self.tx.take();
        }
    }
}
