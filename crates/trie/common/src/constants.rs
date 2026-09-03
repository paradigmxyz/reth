/// The maximum size of a legacy Ethereum trie account's RLP encoding in bytes.
///
/// Extended accounts may exceed this value; it is used only as an initial allocation capacity.
/// 2 (header) + 4 * 1 (field lens) + 8 (nonce) + 32 * 3 (balance, storage root, code hash)
pub const TRIE_ACCOUNT_RLP_MAX_SIZE: usize = 110;

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
            extension: (),
        };
        let mut encoded = Vec::new();
        account.encode(&mut encoded);
        assert_eq!(encoded.len(), TRIE_ACCOUNT_RLP_MAX_SIZE);
    }
}
