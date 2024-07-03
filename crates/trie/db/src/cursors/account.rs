use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::BlockNumberAddress,
    DatabaseError,
};
use reth_primitives::{Account, Address, BlockNumber, B256, U256};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorMut, TrieCursorRw, TrieRangeWalker},
    BranchNodeCompact, Nibbles, StoredNibbles,
};
use reth_trie_common::StoredBranchNode;
use std::ops::RangeInclusive;

/// A cursor over the account trie.
#[derive(Debug)]
pub struct DatabaseAccountTrieCursor<C>(C);

impl<C> DatabaseAccountTrieCursor<C> {
    /// Create a new account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieCursor for DatabaseAccountTrieCursor<C>
where
    C: DbCursorRO<tables::AccountsTrie> + Send + Sync,
{
    type Err = DatabaseError;

    /// Seeks an exact match for the provided key in the account trie.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek_exact(StoredNibbles(key))?.map(|value| (value.0 .0, value.1 .0)))
    }

    /// Seeks a key in the account trie that matches or is greater than the provided key.
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek(StoredNibbles(key))?.map(|value| (value.0 .0, value.1 .0)))
    }

    /// Retrieves the current key in the cursor.
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.0.current()?.map(|(k, _)| k.0))
    }
}

impl<C> TrieCursorMut for DatabaseAccountTrieCursor<C>
where
    C: DbCursorRW<tables::AccountsTrie> + Send + Sync,
{
    type Err = DatabaseError;

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        self.0.delete_current()
    }

    fn upsert(&mut self, key: Nibbles, node: BranchNodeCompact) -> Result<(), DatabaseError> {
        self.0.upsert(StoredNibbles(key), StoredBranchNode(node))
    }
}

impl<C> TrieCursorRw<<Self as TrieCursor>::Err, <Self as TrieCursorMut>::Err>
    for DatabaseAccountTrieCursor<C>
where
    C: DbCursorRW<tables::AccountsTrie> + DbCursorRO<tables::AccountsTrie> + Send + Sync,
{
}

/// A cursor over the account change sets.
#[derive(Debug)]
pub struct DatabaseAccountChangeSetsCursor<C>(C);

impl<C> DatabaseAccountChangeSetsCursor<C> {
    /// Create a new account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieRangeWalker<(Address, Option<Account>)> for DatabaseAccountChangeSetsCursor<C>
where
    C: DbCursorRO<tables::AccountChangeSets> + Send + Sync,
{
    type Err = DatabaseError;

    fn walk_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<impl Iterator<Item = Result<(Address, Option<Account>), Self::Err>>, Self::Err>
    {
        Ok(self.0.walk_range(range)?.map(|v| v.map(|v| (v.1.address, v.1.info))))
    }
}

/// A cursor over the account change sets.
#[derive(Debug)]
pub struct DatabaseStorageChangeSetsCursor<C>(C);

impl<C> DatabaseStorageChangeSetsCursor<C> {
    /// Create a new account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieRangeWalker<(Address, B256, U256)> for DatabaseStorageChangeSetsCursor<C>
where
    C: DbCursorRO<tables::StorageChangeSets> + Send + Sync,
{
    type Err = DatabaseError;

    fn walk_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<impl Iterator<Item = Result<(Address, B256, U256), Self::Err>>, Self::Err> {
        Ok(self
            .0
            .walk_range(BlockNumberAddress::range(range))?
            .map(|v| v.map(|v| (v.0 .0 .1, v.1.key, v.1.value))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db_api::{cursor::DbCursorRW, transaction::DbTxMut};
    use reth_primitives::hex_literal::hex;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_trie::StoredBranchNode;

    #[test]
    fn test_account_trie_order() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();

        let data = vec![
            hex!("0303040e").to_vec(),
            hex!("030305").to_vec(),
            hex!("03030500").to_vec(),
            hex!("0303050a").to_vec(),
        ];

        for key in data.clone() {
            cursor
                .upsert(
                    key.into(),
                    StoredBranchNode(BranchNodeCompact::new(
                        0b0000_0010_0000_0001,
                        0b0000_0010_0000_0001,
                        0,
                        Vec::default(),
                        None,
                    )),
                )
                .unwrap();
        }

        let db_data = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(db_data[0].0 .0.to_vec(), data[0]);
        assert_eq!(db_data[1].0 .0.to_vec(), data[1]);
        assert_eq!(db_data[2].0 .0.to_vec(), data[2]);
        assert_eq!(db_data[3].0 .0.to_vec(), data[3]);

        assert_eq!(
            cursor.seek(hex!("0303040f").to_vec().into()).unwrap().map(|(k, _)| k.0.to_vec()),
            Some(data[1].clone())
        );
    }
}
