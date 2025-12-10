use alloy_primitives::B256;
use reth_trie_common::Nibbles;

/// Target describes a proof target. For every proof target given, the [`crate::ProofCalculator`]
/// will calculate and return all nodes whose path is a prefix of the target's `key`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    pub(crate) key: Nibbles,
    pub(crate) min_len: u8,
}

impl Target {
    /// Returns a new [`Target`] which matches all trie nodes whose path is a prefix of this key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key, min_len: 0 }
    }

    /// Only match trie nodes whose path is at least this long.
    pub fn with_min_len(mut self, min_len: u8) -> Self {
        debug_assert!(min_len <= 64);
        self.min_len = min_len;
        self
    }
}

impl From<B256> for Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}
