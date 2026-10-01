//! MDBX cursors for complete state trie tables.
use alloy_primitives::{B256, U256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_trie::state_trie_cursor::{
    StateTrieCursor, StateTrieCursorFactory, StateTrieStorageCursor,
};
use reth_trie_common::{Nibbles, StateTrieNode, TrieAccount};

/// Creates complete trie cursors within a database transaction.
#[derive(Debug, Clone, Copy)]
pub struct DatabaseStateTrieCursorFactory<T>(pub T);

impl<T: DbTx> StateTrieCursorFactory for DatabaseStateTrieCursorFactory<&T> {
    type AccountCursor<'a>
        = DatabaseStateTrieAccountCursor<T::Cursor<tables::StateTrieAccounts>>
    where
        Self: 'a;
    type StorageCursor<'a>
        = DatabaseStateTrieStorageCursor<T::DupCursor<tables::StateTrieStorages>>
    where
        Self: 'a;
    fn state_trie_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(DatabaseStateTrieAccountCursor(self.0.cursor_read()?))
    }
    fn state_trie_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(DatabaseStateTrieStorageCursor { cursor: self.0.cursor_dup_read()?, address })
    }
}

/// Account trie database cursor.
#[derive(Debug)]
pub struct DatabaseStateTrieAccountCursor<C>(pub C);
impl<C: DbCursorRO<tables::StateTrieAccounts>> StateTrieCursor
    for DatabaseStateTrieAccountCursor<C>
{
    type Value = TrieAccount;
    fn get(&mut self, path: Nibbles) -> Result<Option<StateTrieNode<TrieAccount>>, DatabaseError> {
        Ok(self.0.seek_exact(path.into())?.map(|(_, n)| n))
    }
    fn seek(
        &mut self,
        path: Nibbles,
    ) -> Result<Option<(Nibbles, StateTrieNode<TrieAccount>)>, DatabaseError> {
        Ok(self.0.seek(path.into())?.map(|(p, n)| (p.0, n)))
    }
    fn before(
        &mut self,
        path: Option<Nibbles>,
    ) -> Result<Option<(Nibbles, StateTrieNode<TrieAccount>)>, DatabaseError> {
        let row = match path {
            Some(path) if self.0.seek(path.into())?.is_some() => self.0.prev()?,
            _ => self.0.last()?,
        };
        Ok(row.map(|(p, n)| (p.0, n)))
    }
}

/// Storage trie database cursor.
#[derive(Debug)]
pub struct DatabaseStateTrieStorageCursor<C> {
    cursor: C,
    address: B256,
}
impl<C: DbDupCursorRO<tables::StateTrieStorages> + DbCursorRO<tables::StateTrieStorages>>
    StateTrieCursor for DatabaseStateTrieStorageCursor<C>
{
    type Value = U256;
    fn get(&mut self, path: Nibbles) -> Result<Option<StateTrieNode<U256>>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.address, path.into())?
            .filter(|e| e.nibbles.0 == path)
            .map(|e| e.node))
    }
    fn seek(
        &mut self,
        path: Nibbles,
    ) -> Result<Option<(Nibbles, StateTrieNode<U256>)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.address, path.into())?
            .map(|e| (e.nibbles.0, e.node)))
    }
    fn before(
        &mut self,
        path: Option<Nibbles>,
    ) -> Result<Option<(Nibbles, StateTrieNode<U256>)>, DatabaseError> {
        if let Some(path) = path &&
            self.cursor.seek_by_key_subkey(self.address, path.into())?.is_some()
        {
            return Ok(self.cursor.prev_dup()?.map(|(_, e)| (e.nibbles.0, e.node)))
        }
        if self.cursor.seek_exact(self.address)?.is_none() {
            return Ok(None)
        }
        Ok(self.cursor.last_dup()?.map(|e| (e.nibbles.0, e.node)))
    }
}
impl<C: DbDupCursorRO<tables::StateTrieStorages> + DbCursorRO<tables::StateTrieStorages>>
    StateTrieStorageCursor for DatabaseStateTrieStorageCursor<C>
{
    fn set_hashed_address(&mut self, address: B256) {
        self.address = address;
    }
}

/// Apply complete trie changes, replacing existing duplicate values by path.
pub fn write_state_trie_updates<T: reth_db_api::transaction::DbTxMut>(
    tx: &T,
    updates: &reth_trie_common::StateTrieUpdatesSorted,
) -> Result<(), DatabaseError> {
    use reth_db_api::cursor::DbCursorRW;
    if updates.is_empty() {
        return Ok(())
    }
    for (path, node) in &updates.account_nodes {
        match node {
            Some(node) => tx.put::<tables::StateTrieAccounts>((*path).into(), node.clone())?,
            None => {
                tx.delete::<tables::StateTrieAccounts>((*path).into(), None)?;
            }
        }
    }
    let mut cursor = tx.cursor_dup_write::<tables::StateTrieStorages>()?;
    for (address, nodes) in &updates.storage_tries {
        for (path, node) in nodes {
            if cursor
                .seek_by_key_subkey(*address, (*path).into())?
                .is_some_and(|e| e.nibbles.0 == *path)
            {
                cursor.delete_current()?;
            }
            if let Some(node) = node {
                cursor.upsert(
                    *address,
                    &reth_trie_common::StateTrieStorageEntry {
                        nibbles: (*path).into(),
                        node: node.clone(),
                    },
                )?;
            }
        }
    }
    Ok(())
}
