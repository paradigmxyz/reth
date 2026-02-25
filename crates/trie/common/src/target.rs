use crate::Nibbles;
use alloy_primitives::B256;

/// Target describes a proof target. For every proof target given, a proof calculator will calculate
/// and return all nodes whose path is a prefix of the target's `key_nibbles`.
#[derive(Debug, Copy, Clone)]
pub struct ProofV2Target {
    /// The key of the proof target, as nibbles.
    pub key_nibbles: Nibbles,
    /// The minimum length of a node's path for it to be retained.
    pub min_len: u8,
}

impl ProofV2Target {
    /// Returns a new [`ProofV2Target`] which matches all trie nodes whose path is a prefix of this
    /// key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key_nibbles = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key_nibbles, min_len: 0 }
    }

    /// Returns the key the target was initialized with.
    pub fn key(&self) -> B256 {
        B256::from_slice(&self.key_nibbles.pack())
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
}

impl From<B256> for ProofV2Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}
