#[cfg(feature = "account-ext")]
use crate::AccountExtension;
use crate::{EMPTY_ROOT_HASH, KECCAK_EMPTY};
use alloy_primitives::{B256, U256, keccak256};
use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, Result};

/// Represents an TrieAccount in the account trie.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "account-ext"), derive(Copy))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct TrieAccount {
    /// The account's nonce.
    #[cfg_attr(feature = "serde", serde(with = "quantity"))]
    pub nonce: u64,
    /// The account's balance.
    pub balance: U256,
    /// The hash of the storage account data.
    pub storage_root: B256,
    /// The hash of the code of the account.
    pub code_hash: B256,
    /// Raw chain-specific bytes, encoded as a fifth RLP string only when nonempty.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "AccountExtension::is_empty")
    )]
    #[cfg(feature = "account-ext")]
    pub extension: AccountExtension,
}

impl Default for TrieAccount {
    fn default() -> Self {
        Self {
            nonce: 0,
            balance: U256::ZERO,
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK_EMPTY,
            #[cfg(feature = "account-ext")]
            extension: AccountExtension::default(),
        }
    }
}

impl TrieAccount {
    /// Creates a trie account with an empty extension.
    ///
    /// The extension carries chain-specific data and only exists with the `account-ext` feature.
    /// Use `with_extension` to attach one.
    pub const fn new(nonce: u64, balance: U256, storage_root: B256, code_hash: B256) -> Self {
        Self {
            nonce,
            balance,
            storage_root,
            code_hash,
            #[cfg(feature = "account-ext")]
            extension: AccountExtension::new(),
        }
    }

    /// Returns this trie account with the given chain-specific extension.
    ///
    /// This replaces the extension, which [`Self::new`] leaves empty.
    #[cfg(feature = "account-ext")]
    pub fn with_extension(mut self, extension: impl Into<AccountExtension>) -> Self {
        self.extension = extension.into();
        self
    }

    /// Compute  hash as committed to in the MPT trie without memorizing.
    pub fn trie_hash_slow(&self) -> B256 {
        keccak256(alloy_rlp::encode(self))
    }
}

impl Encodable for TrieAccount {
    fn encode(&self, out: &mut dyn BufMut) {
        let payload_length = self.nonce.length()
            + self.balance.length()
            + self.storage_root.length()
            + self.code_hash.length();
        #[cfg(feature = "account-ext")]
        let payload_length = payload_length
            + if self.extension.is_empty() {
                0
            } else {
                self.extension.rlp_length()
            };
        Header {
            list: true,
            payload_length,
        }
        .encode(out);
        self.nonce.encode(out);
        self.balance.encode(out);
        self.storage_root.encode(out);
        self.code_hash.encode(out);
        #[cfg(feature = "account-ext")]
        if !self.extension.is_empty() {
            self.extension.rlp_encode(out);
        }
    }

    fn length(&self) -> usize {
        let payload_length = self.nonce.length()
            + self.balance.length()
            + self.storage_root.length()
            + self.code_hash.length();
        #[cfg(feature = "account-ext")]
        let payload_length = payload_length
            + if self.extension.is_empty() {
                0
            } else {
                self.extension.rlp_length()
            };
        Header {
            list: true,
            payload_length,
        }
        .length()
            + payload_length
    }
}

impl Decodable for TrieAccount {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(Error::UnexpectedString);
        }

        let (mut payload, rest) = buf.split_at(header.payload_length);
        *buf = rest;
        let account = Self {
            nonce: Decodable::decode(&mut payload)?,
            balance: Decodable::decode(&mut payload)?,
            storage_root: Decodable::decode(&mut payload)?,
            code_hash: Decodable::decode(&mut payload)?,
            #[cfg(feature = "account-ext")]
            extension: {
                if payload.is_empty() {
                    AccountExtension::default()
                } else {
                    AccountExtension::rlp_decode(&mut payload)?
                }
            },
        };
        if !payload.is_empty() {
            return Err(Error::UnexpectedLength);
        }
        Ok(account)
    }
}

#[cfg(feature = "serde")]
mod quantity {
    use alloy_primitives::U64;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        U64::from(*value).serialize(serializer)
    }

    /// Deserializes a primitive number from a "quantity" hex string.
    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        U64::deserialize(deserializer).map(|value| value.to())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rlp::RlpEncodable;

    #[derive(RlpEncodable)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
    struct LegacyAccount {
        #[cfg_attr(feature = "serde", serde(with = "quantity"))]
        nonce: u64,
        balance: U256,
        storage_root: B256,
        code_hash: B256,
    }

    #[test]
    fn empty_extension_preserves_rlp() {
        let account = TrieAccount {
            nonce: 7,
            balance: U256::from(42),
            ..Default::default()
        };
        let legacy = LegacyAccount {
            nonce: account.nonce,
            balance: account.balance,
            storage_root: account.storage_root,
            code_hash: account.code_hash,
        };
        let encoded = alloy_rlp::encode(&account);
        assert_eq!(encoded, alloy_rlp::encode(legacy));
        assert_eq!(encoded.len(), account.length());
        assert_eq!(
            TrieAccount::decode(&mut encoded.as_slice()).unwrap(),
            account
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn account_messagepack_compatibility() {
        let account = TrieAccount {
            nonce: 7,
            balance: U256::from(42),
            ..Default::default()
        };
        let legacy = LegacyAccount {
            nonce: account.nonce,
            balance: account.balance,
            storage_root: account.storage_root,
            code_hash: account.code_hash,
        };
        let encoded = rmp_serde::to_vec(&account).unwrap();
        assert_eq!(encoded, rmp_serde::to_vec(&legacy).unwrap());
        assert_eq!(
            rmp_serde::from_slice::<TrieAccount>(&encoded).unwrap(),
            account
        );
        let decoded: LegacyAccount = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(rmp_serde::to_vec(&decoded).unwrap(), encoded);

        let accounts = alloc::vec![
            account,
            TrieAccount {
                nonce: 9,
                #[cfg(feature = "account-ext")]
                extension: AccountExtension::copy_from_slice(&[0x82, 0xaa]),
                ..Default::default()
            },
            TrieAccount::default(),
        ];
        let record = (accounts, 99u64);
        let encoded = rmp_serde::to_vec(&record).unwrap();
        let decoded: (alloc::vec::Vec<TrieAccount>, u64) = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(decoded, record);
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn extension_is_encoded_as_one_string() {
        let account = TrieAccount {
            extension: AccountExtension::copy_from_slice(&[0x01, 0x82, 0xaa, 0xbb]),
            ..Default::default()
        };
        let encoded = alloy_rlp::encode(&account);
        assert!(encoded.ends_with(&[0x84, 0x01, 0x82, 0xaa, 0xbb]));
        assert_eq!(encoded.len(), account.length());
        assert_eq!(
            TrieAccount::decode(&mut encoded.as_slice()).unwrap(),
            account
        );
        assert_ne!(
            account.trie_hash_slow(),
            TrieAccount::default().trie_hash_slow()
        );
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn arbitrary_extension_bytes_roundtrip() {
        let account = TrieAccount {
            extension: AccountExtension::copy_from_slice(&[0x82, 0xaa]),
            ..Default::default()
        };
        let encoded = alloy_rlp::encode(&account);
        assert!(encoded.ends_with(&[0x82, 0x82, 0xaa]));
        assert_eq!(
            TrieAccount::decode(&mut encoded.as_slice()).unwrap(),
            account
        );
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn extension_must_be_one_nonempty_string() {
        let account = TrieAccount::default();
        for suffix in [
            &[0x80][..],
            &[0xc0][..],
            &[0x01, 0x02][..],
            &[0x82, 0xaa][..],
        ] {
            let encoded = alloy_rlp::encode(&account);
            let mut input = encoded.as_slice();
            let header = Header::decode(&mut input).unwrap();
            let mut invalid = alloc::vec::Vec::new();
            Header {
                list: true,
                payload_length: header.payload_length + suffix.len(),
            }
            .encode(&mut invalid);
            invalid.extend_from_slice(input);
            invalid.extend_from_slice(suffix);
            assert!(alloy_rlp::decode_exact::<TrieAccount>(&invalid).is_err());
        }
    }

    #[test]
    fn new_has_empty_extension() {
        let storage_root = B256::repeat_byte(0x11);
        let code_hash = B256::repeat_byte(0x22);
        let account = TrieAccount::new(7, U256::from(42), storage_root, code_hash);
        assert_eq!(account.nonce, 7);
        assert_eq!(account.balance, U256::from(42));
        assert_eq!(account.storage_root, storage_root);
        assert_eq!(account.code_hash, code_hash);

        // Setting every field would make the update needless without `account-ext`.
        let account = TrieAccount::new(7, U256::from(42), EMPTY_ROOT_HASH, KECCAK_EMPTY);
        let literal = TrieAccount {
            nonce: 7,
            balance: U256::from(42),
            ..Default::default()
        };
        assert_eq!(alloy_rlp::encode(account), alloy_rlp::encode(literal));
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn with_extension_keeps_fields() {
        let storage_root = B256::repeat_byte(0x11);
        let code_hash = B256::repeat_byte(0x22);
        let extension = AccountExtension::copy_from_slice(&[0x82, 0xaa]);
        let account = TrieAccount::new(7, U256::from(42), storage_root, code_hash)
            .with_extension(extension.clone());
        assert_eq!(account.nonce, 7);
        assert_eq!(account.balance, U256::from(42));
        assert_eq!(account.storage_root, storage_root);
        assert_eq!(account.code_hash, code_hash);
        assert_eq!(account.extension, extension);
        let encoded = alloy_rlp::encode(&account);
        assert_eq!(
            alloy_rlp::decode_exact::<TrieAccount>(&encoded).unwrap(),
            account
        );
    }
}

#[cfg(all(test, feature = "account-ext"))]
mod typed_extension_tests {
    use super::*;

    #[test]
    fn typed_and_opaque_accounts_have_disjoint_commitments() {
        let opaque = [0xf8, 1, 0x80, 0x83, 1, 2, 3];
        let typed = TrieAccount::default().with_extension(
            AccountExtension::copy_from_slice(&opaque).with_code_metadata(&[2; 21]),
        );
        let encoded = alloy_rlp::encode(&typed);
        assert_eq!(TrieAccount::decode(&mut encoded.as_slice()).unwrap(), typed);
        let legacy = TrieAccount::default().with_extension(opaque.to_vec());
        assert_ne!(typed.trie_hash_slow(), legacy.trie_hash_slow());
        let encoded = alloy_rlp::encode(&legacy);
        let restored = TrieAccount::decode(&mut encoded.as_slice()).unwrap();
        assert_eq!(restored, legacy);
        assert!(restored.extension.code_metadata().is_none());
        let mut cleared = typed.extension;
        cleared.clear_code_metadata();
        assert_eq!(cleared.as_ref(), opaque);
        assert!(cleared.code_metadata().is_none());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn typed_json_preserves_both_fields() {
        let extension = AccountExtension::copy_from_slice(&[1, 2]).with_code_metadata(&[2; 21]);
        let bytes = serde_json::to_vec(&extension).unwrap();
        assert_eq!(
            serde_json::from_slice::<AccountExtension>(&bytes).unwrap(),
            extension
        );
        let messagepack = rmp_serde::to_vec(&extension).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<AccountExtension>(&messagepack).unwrap(),
            extension
        );
    }
}
