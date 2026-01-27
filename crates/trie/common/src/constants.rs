/// The maximum size of RLP encoded trie account in bytes.
/// 2 (header) + 4 * 1 (field lens) + 8 (nonce) + 32 * 3 (balance, storage root, code hash)
pub const TRIE_ACCOUNT_RLP_MAX_SIZE: usize = 110;

/// Maximum number of account targets per multiproof chunk.
/// This limits the number of accounts processed in a single multiproof job.
pub const MAX_ACCOUNT_TARGETS_PER_CHUNK: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrieAccount;
    use alloy_primitives::{B256, U256};
    use alloy_rlp::Encodable;

    #[test]
    fn account_rlp_max_size() {
        let account = TrieAccount {
            nonce: u64::MAX,
            balance: U256::MAX,
            storage_root: B256::from_slice(&[u8::MAX; 32]),
            code_hash: B256::from_slice(&[u8::MAX; 32]),
        };
        let mut encoded = Vec::new();
        account.encode(&mut encoded);
        assert_eq!(encoded.len(), TRIE_ACCOUNT_RLP_MAX_SIZE);
    }
}
