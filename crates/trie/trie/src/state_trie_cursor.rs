//! Cursors over complete state tries and their in-memory overlays.
use alloy_primitives::{B256, U256};
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{
    Nibbles, StateTrieNode, StateTrieNodes, StateTrieUpdatesSorted, TrieAccount,
};

/// A complete trie cursor, supporting direct parent lookups and ordered neighbor queries.
#[auto_impl::auto_impl(&mut)]
pub trait StateTrieCursor {
    /// Leaf value type.
    type Value: Clone + std::fmt::Debug;
    /// Exact lookup. Missing nodes return `None`.
    fn get(&mut self, path: Nibbles) -> Result<Option<StateTrieNode<Self::Value>>, DatabaseError>;
    /// Exact lookups in input order, including missing nodes.
    fn get_batch(
        &mut self,
        paths: &[Nibbles],
    ) -> Result<Vec<Option<StateTrieNode<Self::Value>>>, DatabaseError> {
        paths.iter().map(|path| self.get(*path)).collect()
    }
    /// First node at or after `path`.
    fn seek(&mut self, path: Nibbles) -> StateTrieCursorResult<Self::Value>;
    /// Last node strictly before `path`, or the final node when no bound is supplied.
    fn before(&mut self, path: Option<Nibbles>) -> StateTrieCursorResult<Self::Value>;
}

/// A storage cursor that can be reused for another account.
pub trait StateTrieStorageCursor: StateTrieCursor<Value = U256> {
    /// Select the storage trie.
    fn set_hashed_address(&mut self, address: B256);
}

/// Creates account and storage cursors over complete tries.
#[auto_impl::auto_impl(&)]
pub trait StateTrieCursorFactory {
    /// Account cursor.
    type AccountCursor<'a>: StateTrieCursor<Value = TrieAccount>
    where
        Self: 'a;
    /// Storage cursor.
    type StorageCursor<'a>: StateTrieStorageCursor
    where
        Self: 'a;
    /// Open the account trie.
    fn state_trie_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError>;
    /// Open an account's storage trie.
    fn state_trie_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError>;
}

/// A sorted overlay whose entries, including deletions, supersede database nodes.
#[derive(Debug)]
pub struct InMemoryStateTrieCursor<'a, C: StateTrieCursor> {
    cursor: C,
    nodes: &'a [(Nibbles, Option<StateTrieNode<C::Value>>)],
    storage: Option<&'a StateTrieUpdatesSorted>,
}

impl<'a, C: StateTrieCursor> InMemoryStateTrieCursor<'a, C> {
    /// Overlay a single trie.
    pub fn new(cursor: C, nodes: &'a StateTrieNodes<C::Value>) -> Self {
        Self { cursor, nodes, storage: None }
    }
}

impl<'a, C: StateTrieStorageCursor> InMemoryStateTrieCursor<'a, C> {
    /// Overlay storage, retaining all accounts so the cursor can switch addresses.
    pub fn new_storage(cursor: C, updates: &'a StateTrieUpdatesSorted, address: B256) -> Self {
        Self {
            cursor,
            nodes: updates.storage_tries.get(&address).map(Vec::as_slice).unwrap_or(&[]),
            storage: Some(updates),
        }
    }
}

impl<C: StateTrieCursor> StateTrieCursor for InMemoryStateTrieCursor<'_, C> {
    type Value = C::Value;
    fn get(&mut self, path: Nibbles) -> Result<Option<StateTrieNode<Self::Value>>, DatabaseError> {
        match self.nodes.binary_search_by_key(&path, |(p, _)| *p) {
            Ok(i) => Ok(self.nodes[i].1.clone()),
            Err(_) => self.cursor.get(path),
        }
    }
    fn seek(&mut self, path: Nibbles) -> StateTrieCursorResult<Self::Value> {
        let index = self.nodes.partition_point(|(p, _)| *p < path);
        let overlay =
            self.nodes[index..].iter().find_map(|(p, n)| n.as_ref().map(|n| (*p, n.clone())));
        if overlay.as_ref().is_some_and(|(p, _)| *p == path) {
            return Ok(overlay)
        }
        let mut db = self.cursor.seek(path)?;
        while let Some((p, _)) = db.as_ref() {
            if overlay.as_ref().is_some_and(|(o, _)| o <= p) {
                return Ok(overlay)
            }
            if self.nodes.binary_search_by_key(p, |(p, _)| *p).is_err() {
                return Ok(db)
            }
            // The next lexicographic nibble path is either its zero child or the next full key.
            let next = if p.len() < 64 {
                let mut p = *p;
                p.push_unchecked(0);
                Some(p)
            } else {
                p.next_without_prefix()
            };
            db = match next {
                Some(next) => self.cursor.seek(next)?,
                None => None,
            };
        }
        Ok(overlay)
    }
    fn before(&mut self, path: Option<Nibbles>) -> StateTrieCursorResult<Self::Value> {
        let index =
            path.map_or(self.nodes.len(), |path| self.nodes.partition_point(|(p, _)| *p < path));
        let overlay =
            self.nodes[..index].iter().rev().find_map(|(p, n)| n.as_ref().map(|n| (*p, n.clone())));
        let mut db = self.cursor.before(path)?;
        while let Some((p, _)) = db.as_ref() {
            if overlay.as_ref().is_some_and(|(o, _)| o >= p) {
                return Ok(overlay)
            }
            if self.nodes.binary_search_by_key(p, |(p, _)| *p).is_err() {
                return Ok(db)
            }
            db = self.cursor.before(Some(*p))?;
        }
        Ok(overlay)
    }
}
impl<C: StateTrieStorageCursor> StateTrieStorageCursor for InMemoryStateTrieCursor<'_, C> {
    fn set_hashed_address(&mut self, address: B256) {
        self.cursor.set_hashed_address(address);
        self.nodes = self
            .storage
            .and_then(|s| s.storage_tries.get(&address))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
    }
}

/// Result of locating a complete trie node and its path.
pub type StateTrieCursorResult<V> = Result<Option<(Nibbles, StateTrieNode<V>)>, DatabaseError>;

#[cfg(test)]
mod tests {
    use super::*;

    struct UnavailableCursor;

    impl StateTrieCursor for UnavailableCursor {
        type Value = U256;

        fn get(&mut self, _: Nibbles) -> Result<Option<StateTrieNode<U256>>, DatabaseError> {
            Err(DatabaseError::Other("database unavailable".into()))
        }

        fn seek(&mut self, _: Nibbles) -> StateTrieCursorResult<U256> {
            Err(DatabaseError::Other("database unavailable".into()))
        }

        fn before(&mut self, _: Option<Nibbles>) -> StateTrieCursorResult<U256> {
            Err(DatabaseError::Other("database unavailable".into()))
        }
    }

    #[test]
    fn exact_overlay_seek_needs_no_database_read() {
        let path = Nibbles::unpack(B256::repeat_byte(1));
        let node = StateTrieNode::Leaf { short_key_len: 64, value: U256::from(1) };
        let nodes = vec![(path, Some(node.clone()))];
        let mut cursor = InMemoryStateTrieCursor::new(UnavailableCursor, &nodes);
        assert_eq!(cursor.seek(path).unwrap(), Some((path, node)));
        // A neighboring overlay entry cannot rule out a closer database node.
        assert!(cursor.seek(Nibbles::unpack(B256::ZERO)).is_err());
        assert!(cursor.seek(Nibbles::unpack(B256::repeat_byte(2))).is_err());

        let nodes = vec![(path, None)];
        let mut cursor = InMemoryStateTrieCursor::new(UnavailableCursor, &nodes);
        // A tombstone still requires finding the next visible database node.
        assert!(cursor.seek(path).is_err());
    }
}
