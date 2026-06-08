use crate::InMemorySize;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_genesis::GenesisAccount;
use alloy_primitives::{keccak256, Bytes, B256, U256};
use alloy_trie::TrieAccount;
use derive_more::Deref;
use thiserror::Error;

#[cfg(feature = "reth-codec")]
/// Identifiers used in [`Compact`](reth_codecs::Compact) encoding of [`Bytecode`].
pub mod compact_ids {
    /// Identifier for legacy raw bytecode.
    pub const LEGACY_RAW_BYTECODE_ID: u8 = 0;

    /// Identifier for removed bytecode variant.
    pub const REMOVED_BYTECODE_ID: u8 = 1;

    /// Identifier for legacy analyzed bytecode.
    pub const LEGACY_ANALYZED_BYTECODE_ID: u8 = 2;

    /// Identifier for EIP-7702 bytecode.
    pub const EIP7702_BYTECODE_ID: u8 = 4;
}

/// An Ethereum account.
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "reth-codec", derive(reth_codecs::Compact))]
#[cfg_attr(feature = "reth-codec", reth_codecs::add_arbitrary_tests(compact))]
pub struct Account {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the account's bytecode.
    pub bytecode_hash: Option<B256>,
}

impl Account {
    /// Whether the account has bytecode.
    pub const fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }

    /// After `SpuriousDragon` empty account is defined as account with nonce == 0 && balance == 0
    /// && bytecode = None (or hash is [`KECCAK_EMPTY`]).
    pub fn is_empty(&self) -> bool {
        self.nonce == 0 &&
            self.balance.is_zero() &&
            self.bytecode_hash.is_none_or(|hash| hash == KECCAK_EMPTY)
    }

    /// Returns an account bytecode's hash.
    /// In case of no bytecode, returns [`KECCAK_EMPTY`].
    pub fn get_bytecode_hash(&self) -> B256 {
        self.bytecode_hash.unwrap_or(KECCAK_EMPTY)
    }

    /// Converts the account into a trie account with the given storage root.
    pub fn into_trie_account(self, storage_root: B256) -> TrieAccount {
        let Self { nonce, balance, bytecode_hash } = self;
        TrieAccount {
            nonce,
            balance,
            storage_root,
            code_hash: bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

impl From<TrieAccount> for Account {
    fn from(value: TrieAccount) -> Self {
        Self {
            balance: value.balance,
            nonce: value.nonce,
            bytecode_hash: (value.code_hash != KECCAK_EMPTY).then_some(value.code_hash),
        }
    }
}

impl InMemorySize for Account {
    fn size(&self) -> usize {
        size_of::<Self>()
    }
}

#[cfg(feature = "reth-codec")]
reth_codecs::impl_compression_for_compact!(Account);

/// Bytecode for an account.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deref)]
pub struct Bytecode(pub Bytes);

/// Bytecode decode error.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("invalid bytecode")]
pub struct BytecodeDecodeError;

impl Bytecode {
    /// Create new bytecode from raw bytes.
    ///
    /// No analysis will be performed.
    ///
    /// # Panics
    ///
    /// Panics if bytecode is EOF and has incorrect format.
    pub fn new_raw(bytes: Bytes) -> Self {
        Self(bytes)
    }

    /// Creates new raw bytecode.
    ///
    /// Returns an error on incorrect Bytecode format.
    #[inline]
    pub fn new_raw_checked(bytecode: Bytes) -> Result<Self, BytecodeDecodeError> {
        Ok(Self::new_raw(bytecode))
    }

    /// Returns the original bytecode bytes.
    #[inline]
    pub fn original_bytes(&self) -> Bytes {
        self.0.clone()
    }

    /// Returns the original bytecode bytes by reference.
    #[inline]
    pub const fn bytes_ref(&self) -> &Bytes {
        &self.0
    }

    /// Returns the bytecode length.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the bytecode is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns `true` if this is EIP-7702 delegation bytecode.
    #[inline]
    pub fn is_eip7702(&self) -> bool {
        self.0.len() == 23 && self.0.starts_with(&[0xef, 0x01, 0x00])
    }

    /// Returns the keccak hash of the bytecode.
    #[inline]
    pub fn hash_slow(&self) -> B256 {
        keccak256(&self.0)
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for Bytecode {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        use compact_ids::LEGACY_RAW_BYTECODE_ID;

        let bytecode = self.bytes_ref();
        buf.put_u32(bytecode.len() as u32);
        buf.put_slice(bytecode.as_ref());
        buf.put_u8(LEGACY_RAW_BYTECODE_ID);
        bytecode.len() + 5
    }

    /// # Panics
    ///
    /// Panics if database contents contain a removed or unknown bytecode variant.
    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        use byteorder::ReadBytesExt;
        use bytes::Buf;

        use compact_ids::*;

        let len = buf.read_u32::<byteorder::BigEndian>().expect("could not read bytecode length")
            as usize;
        let bytes = Bytes::from(buf.copy_to_bytes(len));
        let variant = buf.read_u8().expect("could not read bytecode variant");
        let decoded = match variant {
            LEGACY_RAW_BYTECODE_ID => Self::new_raw(bytes),
            REMOVED_BYTECODE_ID => {
                unreachable!("Junk data in database: checked Bytecode variant was removed")
            }
            LEGACY_ANALYZED_BYTECODE_ID => {
                let original_len = buf.read_u64::<byteorder::BigEndian>().unwrap() as usize;
                // When saving jumptable, its length is getting aligned to u8 boundary. Thus, we
                // need to re-calculate the internal length of bitvec and truncate it when loading
                // jumptables to avoid inconsistencies during `Compact` roundtrip.
                let jump_table_len = if buf.len() * 8 >= bytes.len() {
                    // Use length of padded bytecode if we can fit it
                    bytes.len()
                } else {
                    // Otherwise, use original_len
                    original_len
                };
                buf.advance(jump_table_len.div_ceil(8));
                assert!(
                    original_len <= bytes.len(),
                    "analyzed bytecode original length exceeds bytecode length"
                );
                Self::new_raw(bytes.slice(..original_len))
            }
            EIP7702_BYTECODE_ID => {
                // EIP-7702 bytecode objects will be decoded from the raw bytecode
                Self::new_raw(bytes)
            }
            _ => unreachable!("Junk data in database: unknown Bytecode variant"),
        };
        (decoded, &[])
    }
}

#[cfg(feature = "reth-codec")]
reth_codecs::impl_compression_for_compact!(Bytecode);

impl From<&GenesisAccount> for Account {
    fn from(value: &GenesisAccount) -> Self {
        Self {
            nonce: value.nonce.unwrap_or_default(),
            balance: value.balance,
            bytecode_hash: value.code.as_ref().map(keccak256),
        }
    }
}

#[cfg(all(test, feature = "std", feature = "reth-codec"))]
mod tests {
    use super::*;
    use alloy_primitives::{hex_literal::hex, B256, U256};
    use reth_codecs::Compact;

    #[test]
    fn test_account() {
        let mut buf = vec![];
        let mut acc = Account::default();
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 2);

        acc.balance = U256::from(2);
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 3);

        acc.nonce = 2;
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_empty_account() {
        let mut acc = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None };
        // Nonce 0, balance 0, and bytecode hash set to None is considered empty.
        assert!(acc.is_empty());

        acc.bytecode_hash = Some(KECCAK_EMPTY);
        // Nonce 0, balance 0, and bytecode hash set to KECCAK_EMPTY is considered empty.
        assert!(acc.is_empty());

        acc.balance = U256::from(2);
        // Non-zero balance makes it non-empty.
        assert!(!acc.is_empty());

        acc.balance = U256::ZERO;
        acc.nonce = 10;
        // Non-zero nonce makes it non-empty.
        assert!(!acc.is_empty());

        acc.nonce = 0;
        acc.bytecode_hash = Some(B256::from(U256::ZERO));
        // Non-empty bytecode hash makes it non-empty.
        assert!(!acc.is_empty());
    }

    #[test]
    #[ignore]
    fn test_bytecode() {
        let mut buf = vec![];
        let bytecode = Bytecode::new_raw(Bytes::default());
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 14);

        let mut buf = vec![];
        let bytecode = Bytecode::new_raw(Bytes::from(&hex!("ffff")));
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 17);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_analyzed_bytecode_decodes_original_bytes() {
        use compact_ids::LEGACY_ANALYZED_BYTECODE_ID;

        let original = Bytes::from_static(&[0x60, 0x00, 0x5b]);
        let analyzed = Bytes::from_static(&[0x60, 0x00, 0x5b, 0x00]);

        let mut buf = Vec::new();
        buf.extend_from_slice(&(analyzed.len() as u32).to_be_bytes());
        buf.extend_from_slice(&analyzed);
        buf.push(LEGACY_ANALYZED_BYTECODE_ID);
        buf.extend_from_slice(&(original.len() as u64).to_be_bytes());
        buf.push(0);

        let (decoded, remainder) = Bytecode::from_compact(&buf, buf.len());
        assert_eq!(decoded.original_bytes(), original);
        assert_eq!(decoded.hash_slow(), keccak256(&original));
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_account_has_bytecode() {
        // Account with no bytecode (None)
        let acc_no_bytecode = Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None };
        assert!(!acc_no_bytecode.has_bytecode(), "Account should not have bytecode");

        // Account with bytecode hash set to KECCAK_EMPTY (should have bytecode)
        let acc_empty_bytecode =
            Account { nonce: 1, balance: U256::from(1000), bytecode_hash: Some(KECCAK_EMPTY) };
        assert!(acc_empty_bytecode.has_bytecode(), "Account should have bytecode");

        // Account with a non-empty bytecode hash
        let acc_with_bytecode = Account {
            nonce: 1,
            balance: U256::from(1000),
            bytecode_hash: Some(B256::from_slice(&[0x11u8; 32])),
        };
        assert!(acc_with_bytecode.has_bytecode(), "Account should have bytecode");
    }

    #[test]
    fn test_account_get_bytecode_hash() {
        // Account with no bytecode (should return KECCAK_EMPTY)
        let acc_no_bytecode = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None };
        assert_eq!(acc_no_bytecode.get_bytecode_hash(), KECCAK_EMPTY, "Should return KECCAK_EMPTY");

        // Account with bytecode hash set to KECCAK_EMPTY
        let acc_empty_bytecode =
            Account { nonce: 1, balance: U256::from(1000), bytecode_hash: Some(KECCAK_EMPTY) };
        assert_eq!(
            acc_empty_bytecode.get_bytecode_hash(),
            KECCAK_EMPTY,
            "Should return KECCAK_EMPTY"
        );

        // Account with a valid bytecode hash
        let bytecode_hash = B256::from_slice(&[0x11u8; 32]);
        let acc_with_bytecode =
            Account { nonce: 1, balance: U256::from(1000), bytecode_hash: Some(bytecode_hash) };
        assert_eq!(
            acc_with_bytecode.get_bytecode_hash(),
            bytecode_hash,
            "Should return the bytecode hash"
        );
    }
}
