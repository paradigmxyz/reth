use alloy_primitives::B256;

use crate::Nibbles;

/// Target describes a proof target for trie operations.
///
/// For every proof target given, the proof calculator will return all nodes
/// whose path is a prefix of the target's `key`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    /// The full key path (64 nibbles).
    pub key: Nibbles,
    /// Minimum path length required for the proof.
    /// Only match trie nodes whose path is at least this long.
    pub min_len: u8,
}

impl Target {
    /// Returns a new [`Target`] which matches all trie nodes whose path is a prefix of this key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key, min_len: 0 }
    }

    /// Creates a new target from nibbles with a specific min_len.
    #[inline]
    pub const fn from_nibbles(key: Nibbles, min_len: u8) -> Self {
        Self { key, min_len }
    }

    /// Returns the key the target was initialized with.
    pub fn key(&self) -> B256 {
        B256::from_slice(&self.key.pack())
    }

    /// Only match trie nodes whose path is at least this long.
    ///
    /// # Panics
    ///
    /// This method panics if `min_len` is greater than 64.
    pub fn with_min_len(mut self, min_len: u8) -> Self {
        debug_assert!(min_len <= 64);
        self.min_len = min_len;
        self
    }

    /// Returns the sub-trie prefix for this target.
    ///
    /// A target will only match nodes which share the target's prefix, where the target's prefix
    /// is the first `min_len` nibbles of its key. E.g. a target with `key` 0xabcd and `min_len` 2
    /// will only match nodes with prefix 0xab.
    ///
    /// The sub-trie prefix is the target prefix with a nibble truncated, because a branch node
    /// must be constructed at the parent level to know the node exists at that path.
    #[inline]
    pub fn sub_trie_prefix(&self) -> Nibbles {
        let mut sub_trie_prefix = self.key;
        sub_trie_prefix.truncate(self.min_len.saturating_sub(1) as usize);
        sub_trie_prefix
    }
}

impl From<B256> for Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}
