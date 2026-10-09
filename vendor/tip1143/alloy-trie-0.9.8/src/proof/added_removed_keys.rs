use crate::{Nibbles, TrieMask};
use alloy_primitives::{B256, map::B256Set};

/// Provides added and removed keys for an account or storage trie.
///
/// Used by the [`crate::proof::ProofRetainer`] to determine which nodes may be ancestors of newly
/// added or removed leaves. This information allows for generation of more complete proofs which
/// include the nodes necessary for adding and removing leaves from the trie.
#[derive(Debug, Default, Clone)]
pub struct AddedRemovedKeys {
    /// Keys which are known to be removed from the trie.
    removed_keys: B256Set,
    /// Keys which are known to be added to the trie.
    added_keys: B256Set,
    /// Assume that all keys have been added.
    assume_added: bool,
}

impl AsRef<Self> for AddedRemovedKeys {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AddedRemovedKeys {
    /// Sets the `assume_added` flag, which can be used instead of `insert_added` if exact
    /// additions aren't known and you want to optimistically collect all proofs which _might_ be
    /// necessary.
    pub const fn with_assume_added(mut self, assume_added: bool) -> Self {
        self.assume_added = assume_added;
        self
    }

    /// Sets the key as being a removed key. This removes the key from the `added_keys` set if it
    /// was previously inserted into it.
    pub fn insert_removed(&mut self, key: B256) {
        self.added_keys.remove(&key);
        self.removed_keys.insert(key);
    }

    /// Unsets the key as being a removed key.
    pub fn remove_removed(&mut self, key: &B256) {
        self.removed_keys.remove(key);
    }

    /// Sets the key as being an added key. This removes the key from the `removed_keys` set if it
    /// was previously inserted into it.
    pub fn insert_added(&mut self, key: B256) {
        self.removed_keys.remove(&key);
        self.added_keys.insert(key);
    }

    /// Clears all keys which have been added via `insert_added`.
    pub fn clear_added(&mut self) {
        self.added_keys.clear();
    }

    /// Returns true if the given key path is marked as removed.
    pub fn is_removed(&self, path: &B256) -> bool {
        self.removed_keys.contains(path)
    }

    /// Returns true if the given key path is marked as added.
    pub fn is_added(&self, path: &B256) -> bool {
        self.assume_added || self.added_keys.contains(path)
    }

    /// Returns true if the given path prefix is the prefix of an added key.
    pub fn is_prefix_added(&self, prefix: &Nibbles) -> bool {
        self.assume_added
            || self.added_keys.iter().any(|key| {
                let key_nibbles = Nibbles::unpack(key);
                key_nibbles.starts_with(prefix)
            })
    }

    /// Returns a mask containing a bit set for each child of the branch which is a prefix of a
    /// removed leaf.
    pub fn get_removed_mask(&self, branch_path: &Nibbles) -> TrieMask {
        let mut mask = TrieMask::default();
        for key in &self.removed_keys {
            let key_nibbles = Nibbles::unpack(key);
            if key_nibbles.starts_with(branch_path) {
                let child_bit = key_nibbles.get_unchecked(branch_path.len());
                mask.set_bit(child_bit);
            }
        }
        mask
    }
}
