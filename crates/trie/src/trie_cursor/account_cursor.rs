use super::TrieCursor;
use crate::updates::TrieKey;
use reth_db::{cursor::DbCursorRO, tables, DatabaseError};
use reth_primitives::trie::{BranchNodeCompact, Nibbles};

/// A cursor over the account trie.
#[derive(Debug)]
pub struct AccountTrieCursor<C>(C);

impl<C> AccountTrieCursor<C> {
    /// Create a new account trie cursor.
    pub fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieCursor for AccountTrieCursor<C>
where
    C: DbCursorRO<tables::AccountsTrie>,
{
    type Key = Nibbles;

    fn seek_exact(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek_exact(key)?.map(|value| (value.0.to_vec(), value.1)))
    }

    fn seek(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek(key)?.map(|value| (value.0.to_vec(), value.1)))
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(self.0.current()?.map(|(k, _)| TrieKey::AccountNode(k)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        cursor::{DbCursorRO, DbCursorRW},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives::hex_literal::hex;
    use reth_provider::test_utils::create_test_provider_factory;

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
                    BranchNodeCompact::new(
                        0b0000_0010_0000_0001,
                        0b0000_0010_0000_0001,
                        0,
                        Vec::default(),
                        None,
                    ),
                )
                .unwrap();
        }

        let db_data = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(db_data[0].0.to_vec(), data[0]);
        assert_eq!(db_data[1].0.to_vec(), data[1]);
        assert_eq!(db_data[2].0.to_vec(), data[2]);
        assert_eq!(db_data[3].0.to_vec(), data[3]);

        assert_eq!(
            cursor.seek(hex!("0303040f").to_vec().into()).unwrap().map(|(k, _)| k.to_vec()),
            Some(data[1].clone())
        );
    }
}
