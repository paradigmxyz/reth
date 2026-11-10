//! Account data of the snap/2 protocol and its slim trie account encoding.

use alloy_primitives::{B256, KECCAK256_EMPTY, U256};
use alloy_rlp::{BufMut, Decodable, Encodable, Header, RlpDecodable, RlpEncodable};
use alloy_trie::{TrieAccount, EMPTY_ROOT_HASH};
use reth_codecs_derive::add_arbitrary_tests;

/// Account data in the response.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct AccountData {
    /// Hash of the account address (trie path)
    pub hash: B256,
    /// Account fields encoded as a nested RLP list with default hashes elided.
    pub body: SlimAccountBody,
}

impl AccountData {
    /// Converts `account` to snap/2's slim representation.
    pub fn from_trie_account(hash: B256, account: &TrieAccount) -> Self {
        Self { hash, body: account.into() }
    }

    /// Returns the account the trie leaf commits to.
    ///
    /// Default storage roots and code hashes are restored when decoding the slim body.
    #[allow(clippy::clone_on_copy)]
    pub fn trie_account(&self) -> TrieAccount {
        self.body.0.clone()
    }

    /// Consumes the wire value and returns its hashed key with the decoded trie account.
    #[allow(clippy::missing_const_for_fn)]
    pub fn into_trie_entry(self) -> (B256, TrieAccount) {
        (self.hash, self.body.0)
    }
}

/// A trie account encoded with default storage and code hashes replaced by empty byte strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlimAccountBody(TrieAccount);

impl SlimAccountBody {
    fn as_rlp(&self) -> SlimAccountBodyRef<'_> {
        SlimAccountBodyRef {
            nonce: self.0.nonce,
            balance: self.0.balance,
            storage_root: SlimAccountBodyRef::shorten(&self.0.storage_root, EMPTY_ROOT_HASH),
            code_hash: SlimAccountBodyRef::shorten(&self.0.code_hash, KECCAK256_EMPTY),
        }
    }

    /// Restores a dropped field to `empty`, rejecting any length the encoding never produces.
    fn restore(value: &[u8], empty: B256) -> alloy_rlp::Result<B256> {
        match value {
            [] => Ok(empty),
            _ => B256::try_from(value).map_err(|_| alloy_rlp::Error::UnexpectedLength),
        }
    }
}

impl From<&TrieAccount> for SlimAccountBody {
    #[allow(clippy::clone_on_copy)]
    fn from(account: &TrieAccount) -> Self {
        Self(account.clone())
    }
}

impl Encodable for SlimAccountBody {
    fn encode(&self, out: &mut dyn BufMut) {
        self.as_rlp().encode(out);
    }

    fn length(&self) -> usize {
        self.as_rlp().length()
    }
}

impl Decodable for SlimAccountBody {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let mut payload = Header::decode_bytes(buf, true)?;
        let nonce = u64::decode(&mut payload)?;
        let balance = U256::decode(&mut payload)?;
        let storage_root =
            Self::restore(Header::decode_bytes(&mut payload, false)?, EMPTY_ROOT_HASH)?;
        let code_hash = Self::restore(Header::decode_bytes(&mut payload, false)?, KECCAK256_EMPTY)?;
        if !payload.is_empty() {
            return Err(alloy_rlp::Error::UnexpectedLength)
        }
        Ok(Self(TrieAccount::new(nonce, balance, storage_root, code_hash)))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for SlimAccountBody {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let storage_root = if u.arbitrary()? { u.arbitrary()? } else { EMPTY_ROOT_HASH };
        let code_hash = if u.arbitrary()? { u.arbitrary()? } else { KECCAK256_EMPTY };
        Ok(Self(TrieAccount::new(u.arbitrary()?, u.arbitrary()?, storage_root, code_hash)))
    }
}

/// Borrowed encode twin of [`SlimAccountBody`].
#[derive(RlpEncodable)]
struct SlimAccountBodyRef<'a> {
    /// The account's nonce.
    nonce: u64,
    /// The account's balance.
    balance: U256,
    /// Empty when the account has no storage.
    storage_root: &'a [u8],
    /// Empty when the account has no code.
    code_hash: &'a [u8],
}

impl<'a> SlimAccountBodyRef<'a> {
    /// Drops a field that holds its empty default, which is what makes the encoding slim.
    fn shorten(value: &'a B256, empty: B256) -> &'a [u8] {
        if *value == empty {
            &[]
        } else {
            value.as_slice()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::AccountRangeMessage;
    use test_case::test_case;

    fn trie_account(storage_root: B256, code_hash: B256) -> TrieAccount {
        TrieAccount::new(7, U256::from(42), storage_root, code_hash)
    }

    #[test]
    fn account_range_matches_nested_account_wire_encoding() {
        // [request_id, [[hash, [nonce, balance, storage_root, code_hash]]], proof]
        let wire = alloy_primitives::hex!(
            "ea01e7e6a00101010101010101010101010101010101010101010101010101010101010101c4072a8080c0"
        );
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: vec![AccountData::from_trie_account(
                B256::repeat_byte(1),
                &trie_account(EMPTY_ROOT_HASH, KECCAK256_EMPTY),
            )],
            proof: vec![],
        };

        assert_eq!(alloy_rlp::encode(&message), wire);
        assert_eq!(alloy_rlp::decode_exact::<AccountRangeMessage>(&wire).unwrap(), message);
    }

    #[test]
    fn account_data_rejects_byte_string_wrapped_body() {
        let wire = alloy_primitives::hex!(
            "e7a0010101010101010101010101010101010101010101010101010101010101010185c4072a8080"
        );
        assert!(alloy_rlp::decode_exact::<AccountData>(&wire).is_err());
    }

    #[test]
    fn account_data_consumes_only_its_own_list() {
        let account = AccountData::from_trie_account(
            B256::repeat_byte(1),
            &trie_account(B256::repeat_byte(2), B256::repeat_byte(3)),
        );
        let mut wire = alloy_rlp::encode(&account);
        assert_eq!(wire.len(), account.length());
        wire.push(0x80);
        let mut input = wire.as_slice();
        assert_eq!(AccountData::decode(&mut input).unwrap(), account);
        assert_eq!(input, &[0x80]);
    }

    #[test]
    fn account_data_rejects_missing_truncated_and_extra_body_items() {
        for wire in [
            "e1a00101010101010101010101010101010101010101010101010101010101010101",
            "e4a00101010101010101010101010101010101010101010101010101010101010101c4072a",
            "e7a00101010101010101010101010101010101010101010101010101010101010101c4072a808080",
        ] {
            let bytes = alloy_primitives::hex::decode(wire).unwrap();
            assert!(alloy_rlp::decode_exact::<AccountData>(&bytes).is_err());
        }
    }

    #[test]
    fn slim_body_elides_empty_storage_and_code() {
        let account = trie_account(EMPTY_ROOT_HASH, KECCAK256_EMPTY);
        let hash = B256::repeat_byte(1);
        let encoded = AccountData::from_trie_account(hash, &account);

        let body = alloy_rlp::encode(&encoded.body);
        assert_eq!(body, alloy_primitives::hex!("c4072a8080"));
        assert_eq!(alloy_rlp::decode_exact::<SlimAccountBody>(&body).unwrap(), encoded.body);
        assert_eq!(encoded.trie_account(), account);
        assert_eq!(encoded.into_trie_entry(), (hash, account));
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn slim_body_keeps_non_default_storage_and_code() {
        let account = trie_account(B256::repeat_byte(2), B256::repeat_byte(3));
        let encoded = AccountData::from_trie_account(B256::repeat_byte(1), &account);

        let body = alloy_rlp::encode(&encoded.body);
        assert_eq!(body, alloy_rlp::encode(account.clone()));
        assert_eq!(alloy_rlp::decode_exact::<SlimAccountBody>(&body).unwrap(), encoded.body);
        assert_eq!(encoded.trie_account(), account);
    }

    #[test_case(1)]
    #[test_case(16)]
    #[test_case(31)]
    #[test_case(33)]
    fn slim_body_rejects_invalid_hash_lengths(len: usize) {
        let invalid = vec![0xaa; len];
        for (storage_root, code_hash) in
            [(invalid.as_slice(), &[][..]), (&[][..], invalid.as_slice())]
        {
            let body = alloy_rlp::encode(SlimAccountBodyRef {
                nonce: 7,
                balance: U256::from(42),
                storage_root,
                code_hash,
            });
            assert_eq!(
                alloy_rlp::decode_exact::<SlimAccountBody>(&body),
                Err(alloy_rlp::Error::UnexpectedLength)
            );
        }
    }

    #[test_case("c0")]
    #[test_case("c3072a80")]
    #[test_case("c5072a808080")]
    #[test_case("c4072a808000")]
    fn slim_body_rejects_missing_and_extra_fields(wire: &str) {
        let bytes = alloy_primitives::hex::decode(wire).unwrap();
        assert!(alloy_rlp::decode_exact::<SlimAccountBody>(&bytes).is_err());
    }
}
