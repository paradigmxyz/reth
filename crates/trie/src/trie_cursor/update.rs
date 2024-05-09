use super::{TrieCursor, TrieCursorFactory};
use crate::updates::{TrieKey, TrieOp, TrieUpdatesSorted};
use reth_db::DatabaseError;
use reth_primitives::{
    trie::{BranchNodeCompact, Nibbles, StoredNibbles, StoredNibblesSubKey},
    B256,
};
use tracing::debug;

#[derive(Debug, Clone)]
pub struct TrieUpdatesCursorFactory<'a, CF> {
    cursor_factory: CF,
    trie_updates: &'a TrieUpdatesSorted,
}

impl<'a, CF> TrieUpdatesCursorFactory<'a, CF> {
    pub fn new(cursor_factory: CF, trie_updates: &'a TrieUpdatesSorted) -> Self {
        Self { cursor_factory, trie_updates }
    }
}

impl<'a, CF: TrieCursorFactory> TrieCursorFactory for TrieUpdatesCursorFactory<'a, CF> {
    type AccountTrieCursor = TrieUpdatesAccountTrieCursor<'a, CF::AccountTrieCursor>;
    type StorageTrieCursor = TrieUpdatesStorageTrieCursor<'a, CF::StorageTrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        let cursor = self.cursor_factory.account_trie_cursor()?;
        Ok(TrieUpdatesAccountTrieCursor::new(cursor, self.trie_updates))
    }

    fn storage_tries_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        let cursor = self.cursor_factory.storage_tries_cursor(hashed_address)?;
        Ok(TrieUpdatesStorageTrieCursor::new(cursor, hashed_address, self.trie_updates))
    }
}

#[derive(Debug)]
pub struct TrieUpdatesAccountTrieCursor<'a, C> {
    cursor: C,
    trie_updates: &'a TrieUpdatesSorted,
    last_key: Option<TrieKey>,
}

impl<'a, C> TrieUpdatesAccountTrieCursor<'a, C> {
    fn new(cursor: C, trie_updates: &'a TrieUpdatesSorted) -> Self {
        Self { cursor, trie_updates, last_key: None }
    }
}

impl<'a, C: TrieCursor> TrieCursor for TrieUpdatesAccountTrieCursor<'a, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        debug!(
            target: "trie::trie_updates_account_trie_cursor",
            key = ?key,
            "seek_exact",
        );
        if let Some((trie_key, trie_op)) =
            self.trie_updates.find_account_node(&StoredNibbles(key.clone()))
        {
            self.last_key = Some(trie_key);
            debug!(
                target: "trie::trie_updates_account_trie_cursor",
                last_key = ?self.last_key,
                "seek: found",
            );
            match trie_op {
                TrieOp::Update(node) => Ok(Some((key, node))),
                TrieOp::Delete => Ok(None),
            }
        } else {
            let result = self.cursor.seek_exact(key)?;
            self.last_key =
                result.as_ref().map(|(k, _)| TrieKey::AccountNode(StoredNibbles(k.clone())));
            debug!(
                target: "trie::trie_updates_account_trie_cursor",
                last_key = ?self.last_key,
                "seek: found",
            );
            Ok(result)
        }
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        debug!(
            target: "trie::trie_updates_account_trie_cursor",
            key = ?key,
            "seek",
        );
        let stored_nibbles = StoredNibbles(key.clone());
        let trie_update_entry = self
            .trie_updates
            .trie_operations
            .iter()
            .find(|(k, _)| matches!(k, TrieKey::AccountNode(nibbles) if nibbles <= &stored_nibbles))
            .cloned();

        if let Some((trie_key, trie_op)) = trie_update_entry {
            let nibbles = match &trie_key {
                TrieKey::AccountNode(nibbles) => nibbles.clone(),
                _ => panic!("Invalid trie key"),
            };
            self.last_key = Some(trie_key);
            debug!(
                target: "trie::trie_updates_account_trie_cursor",
                last_key = ?self.last_key,
                "seek: found",
            );
            match trie_op {
                TrieOp::Update(node) => return Ok(Some((nibbles.0, node))),
                TrieOp::Delete => return Ok(None),
            }
        }

        let result = self.cursor.seek(key)?;
        self.last_key =
            result.as_ref().map(|(k, _)| TrieKey::AccountNode(StoredNibbles(k.clone())));
        debug!(
            target: "trie::trie_updates_account_trie_cursor",
            last_key = ?self.last_key,
            "seek: found",
        );
        Ok(result)
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        debug!(
            target: "trie::trie_updates_account_trie_cursor",
            last_key = ?self.last_key,
            "current",
        );
        if self.last_key.is_some() {
            Ok(self.last_key.clone())
        } else {
            self.cursor.current()
        }
    }
}

#[derive(Debug)]
pub struct TrieUpdatesStorageTrieCursor<'a, C> {
    cursor: C,
    trie_update_index: usize,
    trie_updates: &'a TrieUpdatesSorted,
    hashed_address: B256,
    last_key: Option<TrieKey>,
}

impl<'a, C> TrieUpdatesStorageTrieCursor<'a, C> {
    fn new(cursor: C, hashed_address: B256, trie_updates: &'a TrieUpdatesSorted) -> Self {
        Self { cursor, trie_updates, trie_update_index: 0, hashed_address, last_key: None }
    }
}

impl<'a, C: TrieCursor> TrieCursor for TrieUpdatesStorageTrieCursor<'a, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        debug!(
            target: "trie::trie_updates_storage_trie_cursor",
            key = ?key,
            hashed_address = %self.hashed_address,
            "seek_exact",
        );
        if let Some((trie_key, trie_op)) = self
            .trie_updates
            .find_storage_node(&self.hashed_address, &StoredNibblesSubKey(key.clone()))
        {
            self.last_key = Some(trie_key);
            debug!(
                target: "trie::trie_updates_account_trie_cursor",
                last_key = ?self.last_key,
                "seek: found",
            );
            match trie_op {
                TrieOp::Update(node) => Ok(Some((key, node))),
                TrieOp::Delete => Ok(None),
            }
        } else {
            let result = self.cursor.seek_exact(key)?;
            self.last_key = result.as_ref().map(|(k, _)| {
                TrieKey::StorageNode(self.hashed_address, StoredNibblesSubKey(k.clone()))
            });
            debug!(
                target: "trie::trie_updates_account_trie_cursor",
                last_key = ?self.last_key,
                "seek: found",
            );
            Ok(result)
        }
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        debug!(
            target: "trie::trie_updates_storage_trie_cursor",
            key = ?key,
            hashed_address = %self.hashed_address,
            "seek",
        );
        let nibbles_sub_key = StoredNibblesSubKey(key.clone());
        let mut trie_update_entry = self.trie_updates.trie_operations.get(self.trie_update_index);
        while trie_update_entry
            .filter(|(k, _)| matches!(k, TrieKey::StorageNode(address, nibbles) if address == &self.hashed_address &&  nibbles < &nibbles_sub_key)).is_some()
        {
            self.trie_update_index += 1;
            trie_update_entry = self.trie_updates.trie_operations.get(self.trie_update_index);
        }

        if let Some((trie_key, trie_op)) =
            trie_update_entry.filter(|(k, _)| matches!(k, TrieKey::StorageNode(_, _)))
        {
            let nibbles = match trie_key {
                TrieKey::StorageNode(_, nibbles) => nibbles.clone(),
                _ => panic!("this should not happen!"),
            };
            self.last_key = Some(trie_key.clone());
            debug!(
                target: "trie::trie_updates_account_trie_cursor",
                last_key = ?self.last_key,
                "seek: found",
            );
            match trie_op {
                TrieOp::Update(node) => return Ok(Some((nibbles.0, node.clone()))),
                TrieOp::Delete => return Ok(None),
            }
        }

        let result = self.cursor.seek(key)?;
        self.last_key = result.as_ref().map(|(k, _)| {
            TrieKey::StorageNode(self.hashed_address, StoredNibblesSubKey(k.clone()))
        });
        debug!(
            target: "trie::trie_updates_account_trie_cursor",
            last_key = ?self.last_key,
            "seek: found",
        );
        Ok(result)
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        if self.last_key.is_some() {
            Ok(self.last_key.clone())
        } else {
            self.cursor.current()
        }
    }
}
