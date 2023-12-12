//! Helper function for calculating Merkle proofs and hashes.

use crate::{
    constants::EMPTY_OMMER_ROOT_HASH,
    keccak256,
    trie::{HashBuilder, Nibbles, TrieAccount},
    Address, Header, Receipt, ReceiptWithBloom, ReceiptWithBloomRef, TransactionSigned, Withdrawal,
    B256,
};
use alloy_primitives::U256;
use alloy_rlp::Encodable;
use bytes::{BufMut, BytesMut};
use itertools::Itertools;

/// Adjust the index of an item for rlp encoding.
pub const fn adjust_index_for_rlp(i: usize, len: usize) -> usize {
    if i > 0x7f {
        i
    } else if i == 0x7f || i + 1 == len {
        0
    } else {
        i + 1
    }
}

/// Compute a trie root of the collection of rlp encodable items.
pub fn ordered_trie_root<T: Encodable>(items: &[T]) -> B256 {
    ordered_trie_root_with_encoder(items, |item, buf| item.encode(buf))
}

/// Compute a trie root of the collection of items with a custom encoder.
pub fn ordered_trie_root_with_encoder<T, F>(items: &[T], mut encode: F) -> B256
where
    F: FnMut(&T, &mut dyn BufMut),
{
    let mut index_buffer = BytesMut::new();
    let mut value_buffer = BytesMut::new();

    let mut hb = HashBuilder::default();
    let items_len = items.len();
    for i in 0..items_len {
        let index = adjust_index_for_rlp(i, items_len);

        index_buffer.clear();
        index.encode(&mut index_buffer);

        value_buffer.clear();
        encode(&items[index], &mut value_buffer);

        hb.add_leaf(Nibbles::unpack(&index_buffer), &value_buffer);
    }

    hb.root()
}

/// Calculate a transaction root.
///
/// `(rlp(index), encoded(tx))` pairs.
pub fn calculate_transaction_root<T>(transactions: &[T]) -> B256
where
    T: AsRef<TransactionSigned>,
{
    ordered_trie_root_with_encoder(transactions, |tx: &T, buf| tx.as_ref().encode_inner(buf, false))
}

/// Calculates the root hash of the withdrawals.
pub fn calculate_withdrawals_root(withdrawals: &[Withdrawal]) -> B256 {
    ordered_trie_root(withdrawals)
}

/// Calculates the receipt root for a header.
#[cfg(not(feature = "optimism"))]
pub fn calculate_receipt_root(receipts: &[ReceiptWithBloom]) -> B256 {
    ordered_trie_root_with_encoder(receipts, |r, buf| r.encode_inner(buf, false))
}

/// Calculates the receipt root for a header.
#[cfg(feature = "optimism")]
pub fn calculate_receipt_root(
    receipts: &[ReceiptWithBloom],
    chain_spec: &crate::ChainSpec,
    timestamp: u64,
) -> B256 {
    // There is a minor bug in op-geth and op-erigon where in the Regolith hardfork,
    // the receipt root calculation does not include the deposit nonce in the receipt
    // encoding. In the Regolith Hardfork, we must strip the deposit nonce from the
    // receipts before calculating the receipt root. This was corrected in the Canyon
    // hardfork.
    if chain_spec.is_fork_active_at_timestamp(crate::Hardfork::Regolith, timestamp) &&
        !chain_spec.is_fork_active_at_timestamp(crate::Hardfork::Canyon, timestamp)
    {
        let receipts = receipts
            .iter()
            .cloned()
            .map(|mut r| {
                r.receipt.deposit_nonce = None;
                r
            })
            .collect::<Vec<_>>();

        return ordered_trie_root_with_encoder(receipts.as_slice(), |r, buf| {
            r.encode_inner(buf, false)
        })
    }

    ordered_trie_root_with_encoder(receipts, |r, buf| r.encode_inner(buf, false))
}

/// Calculates the receipt root for a header for the reference type of [Receipt].
///
/// NOTE: Prefer [calculate_receipt_root] if you have log blooms memoized.
#[cfg(not(feature = "optimism"))]
pub fn calculate_receipt_root_ref(receipts: &[&Receipt]) -> B256 {
    ordered_trie_root_with_encoder(receipts, |r, buf| {
        ReceiptWithBloomRef::from(*r).encode_inner(buf, false)
    })
}

/// Calculates the receipt root for a header for the reference type of [Receipt].
///
/// NOTE: Prefer [calculate_receipt_root] if you have log blooms memoized.
#[cfg(feature = "optimism")]
pub fn calculate_receipt_root_ref(
    receipts: &[&Receipt],
    chain_spec: &crate::ChainSpec,
    timestamp: u64,
) -> B256 {
    // There is a minor bug in op-geth and op-erigon where in the Regolith hardfork,
    // the receipt root calculation does not include the deposit nonce in the receipt
    // encoding. In the Regolith Hardfork, we must strip the deposit nonce from the
    // receipts before calculating the receipt root. This was corrected in the Canyon
    // hardfork.
    if chain_spec.is_fork_active_at_timestamp(crate::Hardfork::Regolith, timestamp) &&
        !chain_spec.is_fork_active_at_timestamp(crate::Hardfork::Canyon, timestamp)
    {
        let receipts = receipts
            .iter()
            .map(|r| {
                let mut r = (*r).clone();
                r.deposit_nonce = None;
                r
            })
            .collect::<Vec<_>>();

        return ordered_trie_root_with_encoder(&receipts, |r, buf| {
            ReceiptWithBloomRef::from(r).encode_inner(buf, false)
        })
    }

    ordered_trie_root_with_encoder(receipts, |r, buf| {
        ReceiptWithBloomRef::from(*r).encode_inner(buf, false)
    })
}

/// Calculates the root hash for ommer/uncle headers.
pub fn calculate_ommers_root(ommers: &[Header]) -> B256 {
    // Check if `ommers` list is empty
    if ommers.is_empty() {
        return EMPTY_OMMER_ROOT_HASH
    }
    // RLP Encode
    let mut ommers_rlp = Vec::new();
    alloy_rlp::encode_list(ommers, &mut ommers_rlp);
    keccak256(ommers_rlp)
}

/// Hashes and sorts account keys, then proceeds to calculating the root hash of the state
/// represented as MPT.
/// See [state_root_unsorted] for more info.
pub fn state_root_ref_unhashed<'a, A: Into<TrieAccount> + Clone + 'a>(
    state: impl IntoIterator<Item = (&'a Address, &'a A)>,
) -> B256 {
    state_root_unsorted(
        state.into_iter().map(|(address, account)| (keccak256(address), account.clone())),
    )
}

/// Hashes and sorts account keys, then proceeds to calculating the root hash of the state
/// represented as MPT.
/// See [state_root_unsorted] for more info.
pub fn state_root_unhashed<A: Into<TrieAccount>>(
    state: impl IntoIterator<Item = (Address, A)>,
) -> B256 {
    state_root_unsorted(state.into_iter().map(|(address, account)| (keccak256(address), account)))
}

/// Sorts the hashed account keys and calculates the root hash of the state represented as MPT.
/// See [state_root] for more info.
pub fn state_root_unsorted<A: Into<TrieAccount>>(
    state: impl IntoIterator<Item = (B256, A)>,
) -> B256 {
    state_root(state.into_iter().sorted_by_key(|(key, _)| *key))
}

/// Calculates the root hash of the state represented as MPT.
/// Corresponds to [geth's `deriveHash`](https://github.com/ethereum/go-ethereum/blob/6c149fd4ad063f7c24d726a73bc0546badd1bc73/core/genesis.go#L119).
///
/// # Panics
///
/// If the items are not in sorted order.
pub fn state_root<A: Into<TrieAccount>>(state: impl IntoIterator<Item = (B256, A)>) -> B256 {
    let mut hb = HashBuilder::default();
    let mut account_rlp_buf = Vec::new();
    for (hashed_key, account) in state {
        account_rlp_buf.clear();
        account.into().encode(&mut account_rlp_buf);
        hb.add_leaf(Nibbles::unpack(hashed_key), &account_rlp_buf);
    }
    hb.root()
}

/// Hashes storage keys, sorts them and them calculates the root hash of the storage trie.
/// See [storage_root_unsorted] for more info.
pub fn storage_root_unhashed(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    storage_root_unsorted(storage.into_iter().map(|(slot, value)| (keccak256(slot), value)))
}

/// Sorts and calculates the root hash of account storage trie.
/// See [storage_root] for more info.
pub fn storage_root_unsorted(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    storage_root(storage.into_iter().sorted_by_key(|(key, _)| *key))
}

/// Calculates the root hash of account storage trie.
///
/// # Panics
///
/// If the items are not in sorted order.
pub fn storage_root(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    let mut hb = HashBuilder::default();
    for (hashed_slot, value) in storage {
        hb.add_leaf(Nibbles::unpack(hashed_slot), alloy_rlp::encode_fixed_size(&value).as_ref());
    }
    hb.root()
}

/// Implementation of hasher using our keccak256 hashing function
/// for compatibility with `triehash` crate.
#[cfg(any(test, feature = "test-utils"))]
pub mod triehash {
    use super::{keccak256, B256};
    use hash_db::Hasher;
    use plain_hasher::PlainHasher;

    /// A [Hasher] that calculates a keccak256 hash of the given data.
    #[derive(Default, Debug, Clone, PartialEq, Eq)]
    #[non_exhaustive]
    pub struct KeccakHasher;

    #[cfg(any(test, feature = "test-utils"))]
    impl Hasher for KeccakHasher {
        type Out = B256;
        type StdHasher = PlainHasher;

        const LENGTH: usize = 32;

        fn hash(x: &[u8]) -> Self::Out {
            keccak256(x)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bloom,
        constants::EMPTY_ROOT_HASH,
        hex_literal::hex,
        proofs::{calculate_receipt_root, calculate_transaction_root},
        Address, Block, GenesisAccount, Log, Receipt, ReceiptWithBloom, TxType, B256, GOERLI,
        HOLESKY, MAINNET, SEPOLIA, U256,
    };
    use alloy_primitives::b256;
    use alloy_rlp::Decodable;
    use std::collections::HashMap;

    #[test]
    fn check_transaction_root() {
        let data = &hex!("f90262f901f9a092230ce5476ae868e98c7979cfc165a93f8b6ad1922acf2df62e340916efd49da01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa02307107a867056ca33b5087e77c4174f47625e48fb49f1c70ced34890ddd88f3a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba0c598f69a5674cae9337261b669970e24abc0b46e6d284372a239ec8ccbf20b0ab901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8618203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0");
        let block_rlp = &mut data.as_slice();
        let block: Block = Block::decode(block_rlp).unwrap();

        let tx_root = calculate_transaction_root(&block.body);
        assert_eq!(block.transactions_root, tx_root, "Must be the same");
    }

    /// Tests that the receipt root is computed correctly for the regolith block.
    /// This was implemented due to a minor bug in op-geth and op-erigon where in
    /// the Regolith hardfork, the receipt root calculation does not include the
    /// deposit nonce in the receipt encoding.
    /// To fix this an op-reth patch was applied to the receipt root calculation
    /// to strip the deposit nonce from each receipt before calculating the root.
    #[cfg(feature = "optimism")]
    #[test]
    fn check_optimism_receipt_root() {
        use crate::{Bloom, Bytes, OP_GOERLI};

        let cases = [
            // Deposit nonces didn't exist in Bedrock; No need to strip. For the purposes of this
            // test, we do have them, so we should get the same root as Canyon.
            (
                "bedrock",
                1679079599,
                b256!("6eefbb5efb95235476654a8bfbf8cb64a4f5f0b0c80b700b0c5964550beee6d7"),
            ),
            // Deposit nonces introduced in Regolith. They weren't included in the receipt RLP,
            // so we need to strip them - the receipt root will differ.
            (
                "regolith",
                1679079600,
                b256!("e255fed45eae7ede0556fe4fabc77b0d294d18781a5a581cab09127bc4cd9ffb"),
            ),
            // Receipt root hashing bug fixed in Canyon. Back to including the deposit nonce
            // in the receipt RLP when computing the receipt root.
            (
                "canyon",
                1699981200,
                b256!("6eefbb5efb95235476654a8bfbf8cb64a4f5f0b0c80b700b0c5964550beee6d7"),
            ),
        ];

        for case in cases {
            let receipts = vec![
                // 0xb0d6ee650637911394396d81172bd1c637d568ed1fbddab0daddfca399c58b53
                ReceiptWithBloom {
                    receipt: Receipt {
                        tx_type: TxType::DEPOSIT,
                        success: true,
                        cumulative_gas_used: 46913,
                        logs: vec![],
                        #[cfg(feature = "optimism")]
                        deposit_nonce: Some(4012991u64),
                        #[cfg(feature = "optimism")]
                        deposit_receipt_version: None,
                    },
                    bloom: Bloom(hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into()),
                },
                // 0x2f433586bae30573c393adfa02bc81d2a1888a3d6c9869f473fb57245166bd9a
                ReceiptWithBloom {
                    receipt: Receipt {
                        tx_type: TxType::EIP1559,
                        success: true,
                        cumulative_gas_used: 118083,
                        logs: vec![
                            Log {
                                address: hex!("ddb6dcce6b794415145eb5caa6cd335aeda9c272").into(),
                                topics: vec![
                                    b256!("c3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62"),
                                    b256!("000000000000000000000000c498902843af527e674846bb7edefa8ad62b8fb9"),
                                    b256!("000000000000000000000000c498902843af527e674846bb7edefa8ad62b8fb9"),
                                    b256!("0000000000000000000000000000000000000000000000000000000000000000"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000001")),
                            },
                            Log {
                                address: hex!("ddb6dcce6b794415145eb5caa6cd335aeda9c272").into(),
                                topics: vec![
                                    b256!("c3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62"),
                                    b256!("000000000000000000000000c498902843af527e674846bb7edefa8ad62b8fb9"),
                                    b256!("0000000000000000000000000000000000000000000000000000000000000000"),
                                    b256!("000000000000000000000000c498902843af527e674846bb7edefa8ad62b8fb9"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000001")),
                            },
                            Log {
                                address: hex!("ddb6dcce6b794415145eb5caa6cd335aeda9c272").into(),
                                topics: vec![
                                    b256!("0eb774bb9698a73583fe07b6972cf2dcc08d1d97581a22861f45feb86b395820"),
                                    b256!("000000000000000000000000c498902843af527e674846bb7edefa8ad62b8fb9"),
                                    b256!("000000000000000000000000c498902843af527e674846bb7edefa8ad62b8fb9"),
                                ],
                                data: Bytes::from_static(&hex!("0000000000000000000000000000000000000000000000000000000000000003")),
                            },
                        ],
                        #[cfg(feature = "optimism")]
                        deposit_nonce: None,
                        #[cfg(feature = "optimism")]
                        deposit_receipt_version: None,
                    },
                    bloom: Bloom(hex!("00001000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000000000000000020000000000000000000800000000000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000040000000000004000000000080000000000000000000000000000000000000000000000000000008000000000000080020000000000000000000000000002000000000000000000000000000080000010000").into()),
                },
                // 0x6c33676e8f6077f46a62eabab70bc6d1b1b18a624b0739086d77093a1ecf8266
                ReceiptWithBloom {
                    receipt: Receipt {
                        tx_type: TxType::EIP1559,
                        success: true,
                        cumulative_gas_used: 189253,
                        logs: vec![
                            Log {
                                address: hex!("ddb6dcce6b794415145eb5caa6cd335aeda9c272").into(),
                                topics: vec![
                                    b256!("c3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62"),
                                    b256!("0000000000000000000000009d521a04bee134ff8136d2ec957e5bc8c50394ec"),
                                    b256!("0000000000000000000000009d521a04bee134ff8136d2ec957e5bc8c50394ec"),
                                    b256!("0000000000000000000000000000000000000000000000000000000000000000"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000001")),
                            },
                            Log {
                                address: hex!("ddb6dcce6b794415145eb5caa6cd335aeda9c272").into(),
                                topics: vec![
                                    b256!("c3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62"),
                                    b256!("0000000000000000000000009d521a04bee134ff8136d2ec957e5bc8c50394ec"),
                                    b256!("0000000000000000000000000000000000000000000000000000000000000000"),
                                    b256!("0000000000000000000000009d521a04bee134ff8136d2ec957e5bc8c50394ec"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000001")),
                            },
                            Log {
                                address: hex!("ddb6dcce6b794415145eb5caa6cd335aeda9c272").into(),
                                topics: vec![
                                    b256!("0eb774bb9698a73583fe07b6972cf2dcc08d1d97581a22861f45feb86b395820"),
                                    b256!("0000000000000000000000009d521a04bee134ff8136d2ec957e5bc8c50394ec"),
                                    b256!("0000000000000000000000009d521a04bee134ff8136d2ec957e5bc8c50394ec"),
                                ],
                                data: Bytes::from_static(&hex!("0000000000000000000000000000000000000000000000000000000000000003")),
                            },
                        ],
                        #[cfg(feature = "optimism")]
                        deposit_nonce: None,
                        #[cfg(feature = "optimism")]
                        deposit_receipt_version: None,
                    },
                    bloom: Bloom(hex!("00000000000000000000200000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000002000000000020000000000000000000000000000000000000000000000000000000000000000020000000000000000000800000000000000000000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000040000000000004000000000080000000000000000000000000000000000000000000000000000008000000000000080020000000000000000000000000002000000000000000000000000000080000000000").into()),
                },
                // 0x4d3ecbef04ba7ce7f5ab55be0c61978ca97c117d7da448ed9771d4ff0c720a3f
                ReceiptWithBloom {
                    receipt: Receipt {
                        tx_type: TxType::EIP1559,
                        success: true,
                        cumulative_gas_used: 346969,
                        logs: vec![
                            Log {
                                address: hex!("4200000000000000000000000000000000000006").into(),
                                topics: vec![
                                    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"),
                                    b256!("000000000000000000000000c3feb4ef4c2a5af77add15c95bd98f6b43640cc8"),
                                    b256!("0000000000000000000000002992607c1614484fe6d865088e5c048f0650afd4"),
                                ],
                                data: Bytes::from_static(&hex!("0000000000000000000000000000000000000000000000000018de76816d8000")),
                            },
                            Log {
                                address: hex!("cf8e7e6b26f407dee615fc4db18bf829e7aa8c09").into(),
                                topics: vec![
                                    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"),
                                    b256!("0000000000000000000000002992607c1614484fe6d865088e5c048f0650afd4"),
                                    b256!("0000000000000000000000008dbffe4c8bf3caf5deae3a99b50cfcf3648cbc09"),
                                ],
                                data: Bytes::from_static(&hex!("000000000000000000000000000000000000000000000002d24d8e9ac1aa79e2")),
                            },
                            Log {
                                address: hex!("2992607c1614484fe6d865088e5c048f0650afd4").into(),
                                topics: vec![
                                    b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"),
                                ],
                                data: Bytes::from_static(&hex!("000000000000000000000000000000000000000000000009bd50642785c15736000000000000000000000000000000000000000000011bb7ac324f724a29bbbf")),
                            },
                            Log {
                                address: hex!("2992607c1614484fe6d865088e5c048f0650afd4").into(),
                                topics: vec![
                                    b256!("d78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822"),
                                    b256!("00000000000000000000000029843613c7211d014f5dd5718cf32bcd314914cb"),
                                    b256!("0000000000000000000000008dbffe4c8bf3caf5deae3a99b50cfcf3648cbc09"),
                                ],
                                data: Bytes::from_static(&hex!("0000000000000000000000000000000000000000000000000018de76816d800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d24d8e9ac1aa79e2")),
                            },
                            Log {
                                address: hex!("6d0f8d488b669aa9ba2d0f0b7b75a88bf5051cd3").into(),
                                topics: vec![
                                    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"),
                                    b256!("0000000000000000000000008dbffe4c8bf3caf5deae3a99b50cfcf3648cbc09"),
                                    b256!("000000000000000000000000c3feb4ef4c2a5af77add15c95bd98f6b43640cc8"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000000014bc73062aea8093")),
                            },
                            Log {
                                address: hex!("8dbffe4c8bf3caf5deae3a99b50cfcf3648cbc09").into(),
                                topics: vec![
                                    b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000002f122cfadc1ca82a35000000000000000000000000000000000000000000000665879dc0609945d6d1")),
                            },
                            Log {
                                address: hex!("8dbffe4c8bf3caf5deae3a99b50cfcf3648cbc09").into(),
                                topics: vec![
                                    b256!("d78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822"),
                                    b256!("00000000000000000000000029843613c7211d014f5dd5718cf32bcd314914cb"),
                                    b256!("000000000000000000000000c3feb4ef4c2a5af77add15c95bd98f6b43640cc8"),
                                ],
                                data: Bytes::from_static(&hex!("0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d24d8e9ac1aa79e200000000000000000000000000000000000000000000000014bc73062aea80930000000000000000000000000000000000000000000000000000000000000000")),
                            },
                        ],
                        #[cfg(feature = "optimism")]
                        deposit_nonce: None,
                        #[cfg(feature = "optimism")]
                        deposit_receipt_version: None,
                    },
                    bloom: Bloom(hex!("00200000000000000000000080000000000000000000000000040000100004000000000000000000000000100000000000000000000000000000100000000000000000000000000002000008000000200000000200000000020000000000000040000000000000000400000200000000000000000000000000000010000000000400000000010400000000000000000000000000002000c80000004080002000000000000000400200000000800000000000000000000000000000000000000000000002000000000000000000000000000000000100001000000000000000000000002000000000000000000000010000000000000000000000800000800000").into()),
                },
                // 0xf738af5eb00ba23dbc1be2dbce41dbc0180f0085b7fb46646e90bf737af90351
                ReceiptWithBloom {
                    receipt: Receipt {
                        tx_type: TxType::EIP1559,
                        success: true,
                        cumulative_gas_used: 623249,
                        logs: vec![
                            Log {
                                address: hex!("ac6564f3718837caadd42eed742d75c12b90a052").into(),
                                topics: vec![
                                    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"),
                                    b256!("0000000000000000000000000000000000000000000000000000000000000000"),
                                    b256!("000000000000000000000000a4fa7f3fbf0677f254ebdb1646146864c305b76e"),
                                    b256!("000000000000000000000000000000000000000000000000000000000011a1d3"),
                                ],
                                data: Default::default(),
                            },
                            Log {
                                address: hex!("ac6564f3718837caadd42eed742d75c12b90a052").into(),
                                topics: vec![
                                    b256!("9d89e36eadf856db0ad9ffb5a569e07f95634dddd9501141ecf04820484ad0dc"),
                                    b256!("000000000000000000000000a4fa7f3fbf0677f254ebdb1646146864c305b76e"),
                                    b256!("000000000000000000000000000000000000000000000000000000000011a1d3"),
                                ],
                                data: Bytes::from_static(&hex!("00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000037697066733a2f2f516d515141646b33736538396b47716577395256567a316b68643548375562476d4d4a485a62566f386a6d346f4a2f30000000000000000000")),
                            },
                             Log {
                                address: hex!("ac6564f3718837caadd42eed742d75c12b90a052").into(),
                                topics: vec![
                                    b256!("110d160a1bedeea919a88fbc4b2a9fb61b7e664084391b6ca2740db66fef80fe"),
                                    b256!("00000000000000000000000084d47f6eea8f8d87910448325519d1bb45c2972a"),
                                    b256!("000000000000000000000000a4fa7f3fbf0677f254ebdb1646146864c305b76e"),
                                    b256!("000000000000000000000000000000000000000000000000000000000011a1d3"),
                                ],
                                data: Bytes::from_static(&hex!("0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000a4fa7f3fbf0677f254ebdb1646146864c305b76e00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000007717500762343034303661353035646234633961386163316433306335633332303265370000000000000000000000000000000000000000000000000000000000000037697066733a2f2f516d515141646b33736538396b47716577395256567a316b68643548375562476d4d4a485a62566f386a6d346f4a2f30000000000000000000")),
                            },
                        ],
                        #[cfg(feature = "optimism")]
                        deposit_nonce: None,
                        #[cfg(feature = "optimism")]
                        deposit_receipt_version: None,
                    },
                    bloom: Bloom(hex!("00000000000000000000000000000000400000000000000000000000000000000000004000000000000001000000000000000002000000000100000000000000000000000000000000000008000000000000000000000000000000000000000004000000020000000000000000000800000000000000000000000010200100200008000002000000000000000000800000000000000000000002000000000000000000000000000000080000000000000000000000004000000000000000000000000002000000000000000000000000000000000000200000000000000020002000000000000000002000000000000000000000000000000000000000000000").into()),
                },
            ];
            let root = calculate_receipt_root(&receipts, OP_GOERLI.as_ref(), case.1);
            assert_eq!(root, case.2);
        }
    }

    #[test]
    fn check_receipt_root() {
        let logs = vec![Log { address: Address::ZERO, topics: vec![], data: Default::default() }];
        let bloom = bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001");
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::EIP2930,
                success: true,
                cumulative_gas_used: 102068,
                logs,
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
            },
            bloom,
        };
        let receipt = vec![receipt];
        let root = calculate_receipt_root(
            &receipt,
            #[cfg(feature = "optimism")]
            crate::OP_GOERLI.as_ref(),
            #[cfg(feature = "optimism")]
            0,
        );
        assert_eq!(root, b256!("fe70ae4a136d98944951b2123859698d59ad251a381abc9960fa81cae3d0d4a0"));
    }

    #[test]
    fn check_withdrawals_root() {
        // Single withdrawal, amount 0
        // https://github.com/ethereum/tests/blob/9760400e667eba241265016b02644ef62ab55de2/BlockchainTests/EIPTests/bc4895-withdrawals/amountIs0.json
        let data = &hex!("f90238f90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0046119afb1ab36aaa8f66088677ed96cd62762f6d3e65642898e189fbe702d51a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a048a703da164234812273ea083e4ec3d09d028300cd325b46a6a75402e5a7ab95c0c0d9d8808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b80");
        let block: Block = Block::decode(&mut data.as_slice()).unwrap();
        assert!(block.withdrawals.is_some());
        let withdrawals = block.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 1);
        let withdrawals_root = calculate_withdrawals_root(withdrawals);
        assert_eq!(block.withdrawals_root, Some(withdrawals_root));

        // 4 withdrawals, identical indices
        // https://github.com/ethereum/tests/blob/9760400e667eba241265016b02644ef62ab55de2/BlockchainTests/EIPTests/bc4895-withdrawals/twoIdenticalIndex.json
        let data = &hex!("f9028cf90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0ccf7b62d616c2ad7af862d67b9dcd2119a90cebbff8c3cd1e5d7fc99f8755774a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a0a95b9a7b58a6b3cb4001eb0be67951c5517141cb0183a255b5cae027a7b10b36c0c0f86cda808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da018094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710");
        let block: Block = Block::decode(&mut data.as_slice()).unwrap();
        assert!(block.withdrawals.is_some());
        let withdrawals = block.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 4);
        let withdrawals_root = calculate_withdrawals_root(withdrawals);
        assert_eq!(block.withdrawals_root, Some(withdrawals_root));
    }

    #[test]
    fn check_empty_state_root() {
        let genesis_alloc = HashMap::<Address, GenesisAccount>::new();
        let root = state_root_unhashed(genesis_alloc);
        assert_eq!(root, EMPTY_ROOT_HASH);
    }

    #[test]
    fn test_simple_account_state_root() {
        // each fixture specifies an address and expected root hash - the address is initialized
        // with a maximum balance, and is the only account in the state.
        // these test cases are generated by using geth with a custom genesis.json (with a single
        // account that has max balance)
        let fixtures: Vec<(Address, B256)> = vec![
            (
                hex!("9fe4abd71ad081f091bd06dd1c16f7e92927561e").into(),
                hex!("4b35be4231841d212ce2fa43aedbddeadd6eb7d420195664f9f0d55629db8c32").into(),
            ),
            (
                hex!("c2ba9d87f8be0ade00c60d3656c1188e008fbfa2").into(),
                hex!("e1389256c47d63df8856d7729dec9dc2dae074a7f0cbc49acad1cf7b29f7fe94").into(),
            ),
        ];

        for (test_addr, expected_root) in fixtures {
            let mut genesis_alloc = HashMap::new();
            genesis_alloc.insert(
                test_addr,
                GenesisAccount { nonce: None, balance: U256::MAX, code: None, storage: None },
            );
            let root = state_root_unhashed(genesis_alloc);

            assert_eq!(root, expected_root);
        }
    }

    #[test]
    fn test_chain_state_roots() {
        let expected_mainnet_state_root =
            b256!("d7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544");
        let calculated_mainnet_state_root = state_root_ref_unhashed(&MAINNET.genesis.alloc);
        assert_eq!(
            expected_mainnet_state_root, calculated_mainnet_state_root,
            "mainnet state root mismatch"
        );

        let expected_goerli_state_root =
            b256!("5d6cded585e73c4e322c30c2f782a336316f17dd85a4863b9d838d2d4b8b3008");
        let calculated_goerli_state_root = state_root_ref_unhashed(&GOERLI.genesis.alloc);
        assert_eq!(
            expected_goerli_state_root, calculated_goerli_state_root,
            "goerli state root mismatch"
        );

        let expected_sepolia_state_root =
            b256!("5eb6e371a698b8d68f665192350ffcecbbbf322916f4b51bd79bb6887da3f494");
        let calculated_sepolia_state_root = state_root_ref_unhashed(&SEPOLIA.genesis.alloc);
        assert_eq!(
            expected_sepolia_state_root, calculated_sepolia_state_root,
            "sepolia state root mismatch"
        );

        let expected_holesky_state_root =
            b256!("69d8c9d72f6fa4ad42d4702b433707212f90db395eb54dc20bc85de253788783");
        let calculated_holesky_state_root = state_root_ref_unhashed(&HOLESKY.genesis.alloc);
        assert_eq!(
            expected_holesky_state_root, calculated_holesky_state_root,
            "holesky state root mismatch"
        );
    }
}
