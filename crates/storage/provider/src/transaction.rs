use reth_db::{
    common::KeyValue,
    cursor::DbCursorRO,
    database::{Database, DatabaseGAT},
    table::Table,
    transaction::DbTx,
};
use reth_interfaces::{db::DatabaseError as DbError, provider::ProviderError};
use reth_primitives::{BlockHash, BlockNumber, ChainSpec, H256};
use reth_trie::StateRootError;
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// A container for any DB transaction that will open a new inner transaction when the current
/// one is committed.
// NOTE: This container is needed since `Transaction::commit` takes `mut self`, so methods in
// the pipeline that just take a reference will not be able to commit their transaction and let
// the pipeline continue. Is there a better way to do this?
//
// TODO: Re-evaluate if this is actually needed, this was introduced as a way to manage the
// lifetime of the `TXMut` and having a nice API for re-opening a new transaction after `commit`
pub struct Transaction<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
    #[allow(dead_code)]
    chain_spec: Option<Arc<ChainSpec>>,
}

impl<'a, DB: Database> Debug for Transaction<'a, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transaction").finish()
    }
}

impl<'a, DB: Database> Deref for Transaction<'a, DB> {
    type Target = <DB as DatabaseGAT<'a>>::TXMut;

    /// Dereference as the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
    }
}

impl<'a, DB: Database> DerefMut for Transaction<'a, DB> {
    /// Dereference as a mutable reference to the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.tx.as_mut().expect("Tried getting a mutable reference to a non-existent transaction")
    }
}

// === Core impl ===

impl<'this, DB> Transaction<'this, DB>
where
    DB: Database,
{
    /// Create a new container with the given database handle.
    ///
    /// A new inner transaction will be opened.
    pub fn new(db: &'this DB) -> Result<Self, DbError> {
        Ok(Self { db, tx: Some(db.tx_mut()?), chain_spec: None })
    }

    /// Creates a new container with given database and transaction handles.
    pub fn new_raw(db: &'this DB, tx: <DB as DatabaseGAT<'this>>::TXMut) -> Self {
        Self { db, tx: Some(tx), chain_spec: None }
    }

    /// Accessor to the internal Database
    pub fn inner(&self) -> &'this DB {
        self.db
    }

    /// Drops the current inner transaction and open a new one.
    pub fn drop(&mut self) -> Result<(), DbError> {
        if let Some(tx) = self.tx.take() {
            drop(tx);
        }

        self.tx = Some(self.db.tx_mut()?);

        Ok(())
    }

    /// Open a new inner transaction.
    pub fn open(&mut self) -> Result<(), DbError> {
        self.tx = Some(self.db.tx_mut()?);
        Ok(())
    }

    /// Close the current inner transaction.
    pub fn close(&mut self) {
        self.tx.take();
    }

    /// Commit the current inner transaction and open a new one.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    pub fn commit(&mut self) -> Result<bool, DbError> {
        let success = if let Some(tx) = self.tx.take() { tx.commit()? } else { false };
        self.tx = Some(self.db.tx_mut()?);
        Ok(success)
    }
}

// === Stages impl ===

impl<'this, 'a, DB> Transaction<'this, DB>
where
    DB: Database,
    'a: 'this,
{
    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DbError>
    where
        T::Key: Default + Ord,
    {
        self.cursor_read::<T>()?.walk(Some(T::Key::default()))?.collect::<Result<Vec<_>, DbError>>()
    }
}

/// An error that can occur when using the transaction container
#[derive(Debug, PartialEq, Eq, Clone, thiserror::Error)]
pub enum TransactionError {
    /// The transaction encountered a database error.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The transaction encountered a database integrity error.
    #[error(transparent)]
    DatabaseIntegrity(#[from] ProviderError),
    /// The trie error.
    #[error(transparent)]
    TrieError(#[from] StateRootError),
    /// Root mismatch
    #[error("Merkle trie root mismatch at #{block_number} ({block_hash:?}). Got: {got:?}. Expected: {expected:?}")]
    StateRootMismatch {
        /// Expected root
        expected: H256,
        /// Calculated root
        got: H256,
        /// Block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
    /// Root mismatch during unwind
    #[error("Unwind merkle trie root mismatch at #{block_number} ({block_hash:?}). Got: {got:?}. Expected: {expected:?}")]
    UnwindStateRootMismatch {
        /// Expected root
        expected: H256,
        /// Calculated root
        got: H256,
        /// Target block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
}

#[cfg(test)]
mod test {
    use crate::{
        insert_canonical_block, test_utils::blocks::*, ShareableDatabase, Transaction,
        TransactionsProvider,
    };
    use reth_db::mdbx::test_utils::create_test_rw_db;
    use reth_primitives::{ChainSpecBuilder, MAINNET};
    use std::{ops::DerefMut, sync::Arc};

    #[test]
    fn insert_block_and_hashes_get_take() {
        let db = create_test_rw_db();

        // setup
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let chain_spec = ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(MAINNET.genesis.clone())
            .shanghai_activated()
            .build();

        let data = BlockChainTestData::default();
        let genesis = data.genesis.clone();
        let (block1, exec_res1) = data.blocks[0].clone();
        let (block2, exec_res2) = data.blocks[1].clone();

        insert_canonical_block(tx.deref_mut(), data.genesis.clone(), None).unwrap();

        assert_genesis_block(&tx, data.genesis);

        tx.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();

        // get one block
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        let get_block = get[0].0.clone();
        let get_state = get[0].1.clone();
        assert_eq!(get_block, block1);
        assert_eq!(get_state, exec_res1);

        // take one block
        let take = tx.take_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(take, vec![(block1.clone(), exec_res1.clone())]);
        assert_genesis_block(&tx, genesis.clone());

        tx.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();
        tx.append_blocks_with_post_state(vec![block2.clone()], exec_res2.clone()).unwrap();
        tx.commit().unwrap();

        // Check that transactions map onto blocks correctly.
        {
            let provider = ShareableDatabase::new(tx.db, Arc::new(MAINNET.clone()));
            assert_eq!(
                provider.transaction_block(0).unwrap(),
                Some(1),
                "Transaction 0 should be in block 1"
            );
            assert_eq!(
                provider.transaction_block(1).unwrap(),
                Some(2),
                "Transaction 1 should be in block 2"
            );
            assert_eq!(
                provider.transaction_block(2).unwrap(),
                None,
                "Transaction 0 should not exist"
            );
        }

        // get second block
        let get = tx.get_block_and_execution_range(&chain_spec, 2..=2).unwrap();
        assert_eq!(get, vec![(block2.clone(), exec_res2.clone())]);

        // get two blocks
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get[0].0, block1);
        assert_eq!(get[1].0, block2);
        assert_eq!(get[0].1, exec_res1);
        assert_eq!(get[1].1, exec_res2);

        // take two blocks
        let get = tx.take_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get, vec![(block1, exec_res1), (block2, exec_res2)]);

        // assert genesis state
        assert_genesis_block(&tx, genesis);
    }

    #[test]
    fn insert_get_take_multiblocks() {
        let db = create_test_rw_db();

        // setup
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let chain_spec = ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(MAINNET.genesis.clone())
            .shanghai_activated()
            .build();

        let data = BlockChainTestData::default();
        let genesis = data.genesis.clone();
        let (block1, exec_res1) = data.blocks[0].clone();
        let (block2, exec_res2) = data.blocks[1].clone();

        insert_canonical_block(tx.deref_mut(), data.genesis.clone(), None).unwrap();

        assert_genesis_block(&tx, data.genesis);

        tx.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();

        // get one block
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(get, vec![(block1.clone(), exec_res1.clone())]);

        // take one block
        let take = tx.take_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(take, vec![(block1.clone(), exec_res1.clone())]);
        assert_genesis_block(&tx, genesis.clone());

        // insert two blocks
        let mut merged_state = exec_res1.clone();
        merged_state.extend(exec_res2.clone());
        tx.append_blocks_with_post_state(
            vec![block1.clone(), block2.clone()],
            merged_state.clone(),
        )
        .unwrap();

        // get second block
        let get = tx.get_block_and_execution_range(&chain_spec, 2..=2).unwrap();
        assert_eq!(get, vec![(block2.clone(), exec_res2.clone())]);

        // get two blocks
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(
            get,
            vec![(block1.clone(), exec_res1.clone()), (block2.clone(), exec_res2.clone())]
        );

        // take two blocks
        let get = tx.take_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get, vec![(block1, exec_res1), (block2, exec_res2)]);

        // assert genesis state
        assert_genesis_block(&tx, genesis);
    }
}
