//! Implements the `GetBlockAccessLists` and `BlockAccessLists` message types.

use alloc::vec::Vec;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::{
    BufMut, Decodable, Encodable, Header, RlpDecodableWrapper, RlpEncodableWrapper,
    EMPTY_STRING_CODE,
};
use reth_codecs_derive::add_arbitrary_tests;

/// A request for block access lists from the given block hashes.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetBlockAccessLists(
    /// The block hashes to request block access lists for.
    pub Vec<B256>,
);

/// Response for [`GetBlockAccessLists`] containing one BAL entry per requested block hash.
///
/// Present `Bytes` values store raw BAL RLP payloads and are encoded as nested RLP items, not as
/// RLP byte strings.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(rlp)]
pub struct BlockAccessLists(
    /// The requested block access lists as raw RLP blobs. Per EIP-8159, unavailable entries are
    /// represented by `None` and encoded as the RLP empty string (`0x80`).
    pub Vec<Option<Bytes>>,
);

impl Encodable for BlockAccessLists {
    fn encode(&self, out: &mut dyn BufMut) {
        let payload_length =
            self.0.iter().map(|entry| entry.as_ref().map_or(1, |bytes| bytes.len())).sum();
        Header { list: true, payload_length }.encode(out);
        for entry in &self.0 {
            match entry {
                Some(bal) => out.put_slice(bal),
                None => out.put_u8(EMPTY_STRING_CODE),
            }
        }
    }

    fn length(&self) -> usize {
        let payload_length =
            self.0.iter().map(|entry| entry.as_ref().map_or(1, |bytes| bytes.len())).sum();
        Header { list: true, payload_length }.length_with_payload()
    }
}

impl Decodable for BlockAccessLists {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString)
        }
        if buf.len() < header.payload_length {
            return Err(alloy_rlp::Error::InputTooShort)
        }

        let (mut payload, rest) = buf.split_at(header.payload_length);
        *buf = rest;
        let mut bals = Vec::new();

        while !payload.is_empty() {
            let item_start = payload;
            let item_header = Header::decode(&mut payload)?;
            let header_length = item_start.len() - payload.len();
            let item_length = header_length + item_header.payload_length;
            if item_length > item_start.len() {
                return Err(alloy_rlp::Error::InputTooShort)
            }
            if item_header.list {
                bals.push(Some(Bytes::copy_from_slice(&item_start[..item_length])));
            } else if item_start[..item_length] == [EMPTY_STRING_CODE] {
                bals.push(None);
            } else {
                return Err(alloy_rlp::Error::UnexpectedString)
            }

            payload = &payload[item_header.payload_length..];
        }

        Ok(Self(bals))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for BlockAccessLists {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let entries = Vec::<Option<Vec<alloy_eip7928::AccountChanges>>>::arbitrary(u)?
            .into_iter()
            .map(|entry| {
                let entry = entry?;
                let mut out = Vec::new();
                alloy_rlp::encode_list(&entry, &mut out);
                Some(Bytes::from(out))
            })
            .collect();
        Ok(Self(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        AccountChanges, BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges,
        StorageChange,
    };
    use alloy_primitives::{Address, U256};
    use alloy_rlp::{EMPTY_LIST_CODE, EMPTY_STRING_CODE};

    fn elaborate_account_changes(seed: u8) -> Vec<AccountChanges> {
        vec![
            AccountChanges {
                address: Address::from([seed; 20]),
                storage_changes: vec![SlotChanges::new(
                    U256::from_be_bytes([seed.wrapping_add(1); 32]),
                    vec![
                        StorageChange::new(
                            BlockAccessIndex::new(1),
                            U256::from_be_bytes([seed.wrapping_add(2); 32]),
                        ),
                        StorageChange::new(
                            BlockAccessIndex::new(2),
                            U256::from_be_bytes([seed.wrapping_add(3); 32]),
                        ),
                    ],
                )],
                storage_reads: vec![
                    U256::from_be_bytes([seed.wrapping_add(4); 32]),
                    U256::from_be_bytes([seed.wrapping_add(5); 32]),
                ],
                balance_changes: vec![
                    BalanceChange::new(BlockAccessIndex::new(1), U256::from(1_000 + seed as u64)),
                    BalanceChange::new(BlockAccessIndex::new(2), U256::from(2_000 + seed as u64)),
                ],
                nonce_changes: vec![
                    NonceChange::new(BlockAccessIndex::new(1), seed as u64),
                    NonceChange::new(BlockAccessIndex::new(2), seed as u64 + 1),
                ],
                code_changes: vec![CodeChange::new(
                    BlockAccessIndex::new(1),
                    Bytes::from(vec![0x60, seed, 0x61, seed.wrapping_add(1), 0x56]),
                )],
                storage_root: None,
            },
            AccountChanges {
                address: Address::from([seed.wrapping_add(9); 20]),
                storage_changes: Vec::new(),
                storage_reads: vec![U256::from_be_bytes([seed.wrapping_add(10); 32])],
                balance_changes: vec![BalanceChange::new(
                    BlockAccessIndex::new(3),
                    U256::from(3_000 + seed as u64),
                )],
                nonce_changes: vec![NonceChange::new(BlockAccessIndex::new(3), seed as u64 + 2)],
                code_changes: vec![CodeChange::new(
                    BlockAccessIndex::new(2),
                    Bytes::from(vec![0x5f, 0x5f, 0xf3]),
                )],
                storage_root: None,
            },
        ]
    }

    fn elaborate_bal_entry(seed: u8) -> Bytes {
        let account_changes = elaborate_account_changes(seed);
        let mut out = Vec::new();
        alloy_rlp::encode_list(&account_changes, &mut out);
        Bytes::from(out)
    }

    #[test]
    fn unavailable_bal_entry_encodes_as_empty_string() {
        let encoded = alloy_rlp::encode(BlockAccessLists(vec![None]));
        assert_eq!(encoded, vec![0xc1, EMPTY_STRING_CODE]);
    }

    #[test]
    fn empty_bal_entry_encodes_as_empty_list() {
        let encoded =
            alloy_rlp::encode(BlockAccessLists(vec![Some(Bytes::from_static(&[EMPTY_LIST_CODE]))]));
        assert_eq!(encoded, vec![0xc1, EMPTY_LIST_CODE]);
    }

    #[test]
    fn block_access_lists_roundtrip_preserves_raw_bal_items() {
        let original = BlockAccessLists(vec![
            None,
            Some(Bytes::from_static(&[EMPTY_LIST_CODE])),
            Some(Bytes::from_static(&[0xc1, EMPTY_LIST_CODE])),
            Some(Bytes::from_static(&[0xc2, EMPTY_LIST_CODE, EMPTY_LIST_CODE])),
        ]);

        let encoded = alloy_rlp::encode(&original);
        let decoded = alloy_rlp::decode_exact::<BlockAccessLists>(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn empty_response_roundtrips() {
        let original = BlockAccessLists(Vec::new());
        let encoded = alloy_rlp::encode(&original);
        let decoded = alloy_rlp::decode_exact::<BlockAccessLists>(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn rejects_non_list_bal_entries() {
        let err = alloy_rlp::decode_exact::<BlockAccessLists>(&[0xc1, 0x01]).unwrap_err();
        assert!(matches!(err, alloy_rlp::Error::UnexpectedString));
    }

    #[test]
    fn rejects_non_empty_string_bal_entries() {
        let err = alloy_rlp::decode_exact::<BlockAccessLists>(&[0xc2, 0x81, 0x80]).unwrap_err();
        assert!(matches!(err, alloy_rlp::Error::UnexpectedString));
    }

    #[test]
    fn rejects_truncated_response_payload() {
        let err =
            alloy_rlp::decode_exact::<BlockAccessLists>(&[0xc2, EMPTY_LIST_CODE]).unwrap_err();
        assert!(matches!(err, alloy_rlp::Error::InputTooShort));
    }

    #[test]
    fn elaborate_bal_entry_roundtrips_into_account_changes() {
        let expected = elaborate_account_changes(0x11);
        let decoded =
            alloy_rlp::decode_exact::<Vec<AccountChanges>>(&elaborate_bal_entry(0x11)).unwrap();

        assert_eq!(decoded, expected);
    }

    #[test]
    fn elaborate_block_access_lists_roundtrip_preserves_complex_bal_contents() {
        let original = BlockAccessLists(vec![
            Some(elaborate_bal_entry(0x11)),
            None,
            Some(Bytes::from_static(&[EMPTY_LIST_CODE])),
            Some(elaborate_bal_entry(0x77)),
        ]);

        let encoded = alloy_rlp::encode(&original);
        let decoded = alloy_rlp::decode_exact::<BlockAccessLists>(&encoded).unwrap();

        assert_eq!(decoded, original);
        assert_eq!(
            alloy_rlp::decode_exact::<Vec<AccountChanges>>(decoded.0[0].as_ref().unwrap()).unwrap(),
            elaborate_account_changes(0x11)
        );
        assert!(decoded.0[1].is_none());
        assert_eq!(
            alloy_rlp::decode_exact::<Vec<AccountChanges>>(decoded.0[2].as_ref().unwrap()).unwrap(),
            vec![]
        );
        assert_eq!(
            alloy_rlp::decode_exact::<Vec<AccountChanges>>(decoded.0[3].as_ref().unwrap()).unwrap(),
            elaborate_account_changes(0x77)
        );
    }

    #[test]
    fn elaborate_block_access_lists_embed_raw_bal_payloads_without_reencoding() {
        let first = elaborate_bal_entry(0x21);
        let second = elaborate_bal_entry(0x42);
        let encoded =
            alloy_rlp::encode(BlockAccessLists(vec![Some(first.clone()), Some(second.clone())]));

        let header = alloy_rlp::Header::decode(&mut &encoded[..]).unwrap();
        let payload = &encoded[header.length()..];
        let expected_payload = [first.as_ref(), second.as_ref()].concat();

        assert!(header.list);
        assert_eq!(payload, expected_payload.as_slice());
    }
}
