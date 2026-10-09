use crate::InMemorySize;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_genesis::GenesisAccount;
use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_trie::TrieAccount;
use derive_more::Deref;
use revm_bytecode::{Bytecode as RevmBytecode, BytecodeDecodeError};
use revm_state::AccountInfo;

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

/// An Ethereum account with chain-specific extension data.
#[cfg_attr(
    any(test, feature = "serde"),
    derive(serde::Serialize, serde::Deserialize)
)]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(not(feature = "account-ext"), derive(Copy))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Account {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the account's bytecode.
    pub bytecode_hash: Option<B256>,
    /// Chain-specific account data committed to the account trie leaf.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(
            default,
            skip_serializing_if = "alloy_trie::AccountExtension::is_empty"
        )
    )]
    #[cfg(feature = "account-ext")]
    pub extension: alloy_trie::AccountExtension,
}

/// Returned when an operation requires an extensionless account representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("account extensions are not supported")]
pub struct AccountExtensionsUnsupported;

/// Rejects operations that cannot represent chain-specific account payloads.
pub const fn ensure_no_account_extensions() -> Result<(), AccountExtensionsUnsupported> {
    if Account::EXTENSIONS_ENABLED {
        Err(AccountExtensionsUnsupported)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod account_extension_tests {
    use super::{Account, AccountExtensionsUnsupported, ensure_no_account_extensions};
    use alloc::string::ToString;
    use alloy_primitives::{B256, U256};

    #[test]
    fn rejects_extension_builds() {
        assert_eq!(
            ensure_no_account_extensions().is_ok(),
            !Account::EXTENSIONS_ENABLED
        );
    }

    #[test]
    fn unsupported_extensions_display() {
        assert_eq!(
            AccountExtensionsUnsupported.to_string(),
            "account extensions are not supported"
        );
    }

    #[test]
    fn new_has_no_extension() {
        let account = Account::new(7, U256::from(42), Some(B256::repeat_byte(1)));
        assert_eq!(account.nonce, 7);
        assert_eq!(account.balance, U256::from(42));
        assert_eq!(account.bytecode_hash, Some(B256::repeat_byte(1)));
        assert!(!account.has_extension());
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn with_extension_sets_extension() {
        let extension = alloy_trie::AccountExtension::copy_from_slice(&[0x82, 0xaa]);
        let account = Account::new(7, U256::from(42), Some(B256::repeat_byte(1)))
            .with_extension(extension.clone());
        assert_eq!(account.nonce, 7);
        assert_eq!(account.balance, U256::from(42));
        assert_eq!(account.bytecode_hash, Some(B256::repeat_byte(1)));
        assert_eq!(account.extension, extension);
        assert!(account.has_extension());
    }
}

#[cfg(feature = "reth-codec")]
#[derive(reth_codecs::Compact)]
struct LegacyAccount {
    nonce: u64,
    balance: U256,
    bytecode_hash: Option<B256>,
}

impl Account {
    /// Whether this build can carry chain-specific account payloads.
    pub const EXTENSIONS_ENABLED: bool = cfg!(feature = "account-ext");

    /// Creates an account with an empty extension.
    ///
    /// The extension carries chain-specific data and only exists with the `account-ext` feature.
    /// Use `with_extension` to attach one.
    pub const fn new(nonce: u64, balance: U256, bytecode_hash: Option<B256>) -> Self {
        Self {
            nonce,
            balance,
            bytecode_hash,
            #[cfg(feature = "account-ext")]
            extension: alloy_trie::AccountExtension::new(),
        }
    }

    /// Returns this account with the given chain-specific extension.
    ///
    /// This replaces the extension, which [`Self::new`] leaves empty.
    #[cfg(feature = "account-ext")]
    pub fn with_extension(mut self, extension: impl Into<alloy_trie::AccountExtension>) -> Self {
        self.extension = extension.into();
        self
    }

    /// Whether this account has a nonempty chain-specific payload.
    pub const fn has_extension(&self) -> bool {
        #[cfg(feature = "account-ext")]
        {
            !self.extension.is_empty()
        }
        #[cfg(not(feature = "account-ext"))]
        {
            false
        }
    }

    /// Number of bytes used by the backwards-compatible account Compact flags.
    #[cfg(feature = "reth-codec")]
    pub const fn bitflag_encoded_bytes() -> usize {
        LegacyAccount::bitflag_encoded_bytes()
    }

    /// Number of unused bits in the backwards-compatible account Compact flags.
    #[cfg(feature = "reth-codec")]
    pub const fn bitflag_unused_bits() -> usize {
        LegacyAccount::bitflag_unused_bits()
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for Account {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let legacy = LegacyAccount {
            nonce: self.nonce,
            balance: self.balance,
            bytecode_hash: self.bytecode_hash,
        };
        let len = legacy.to_compact(buf);
        #[cfg(feature = "account-ext")]
        {
            if self.extension.is_empty() {
                return len;
            }
            let extension_len = u16::try_from(self.extension.len())
                .expect("account extension exceeds compact encoding limit");
            buf.put_u16(extension_len);
            buf.put_slice(&self.extension);
            if let Some(metadata) = self.extension.code_metadata() {
                // A trailer outside the length-delimited opaque bytes selects the typed
                // representation. No valid old opaque payload can collide with it.
                buf.put_u8(1);
                buf.put_u16(u16::try_from(metadata.len()).expect("code metadata exceeds limit"));
                buf.put_slice(metadata);
                len + 2 + self.extension.len() + 3 + metadata.len()
            } else {
                len + 2 + self.extension.len()
            }
        }
        #[cfg(not(feature = "account-ext"))]
        len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (account_buf, rest) = buf.split_at(len);
        let (legacy, buf) = LegacyAccount::from_compact(account_buf, len);
        #[cfg(feature = "account-ext")]
        let extension = if buf.is_empty() {
            alloy_trie::AccountExtension::default()
        } else {
            let (length, bytes) = buf.split_at(2);
            let extension_len = usize::from(u16::from_be_bytes(length.try_into().unwrap()));
            let (opaque, trailer) = bytes.split_at(extension_len);
            let extension = alloy_trie::AccountExtension::copy_from_slice(opaque);
            if trailer.is_empty() {
                extension
            } else {
                assert!(
                    trailer.len() >= 3 && trailer[0] == 1,
                    "invalid typed account extension"
                );
                let metadata_len = usize::from(u16::from_be_bytes([trailer[1], trailer[2]]));
                assert_eq!(
                    trailer.len(),
                    3 + metadata_len,
                    "invalid code metadata length"
                );
                assert!(
                    metadata_len > 0 && metadata_len <= 4096,
                    "invalid code metadata length"
                );
                extension.with_code_metadata(&trailer[3..])
            }
        };
        #[cfg(not(feature = "account-ext"))]
        assert!(buf.is_empty(), "account extensions require account-ext");
        (
            Self {
                nonce: legacy.nonce,
                balance: legacy.balance,
                bytecode_hash: legacy.bytecode_hash,
                #[cfg(feature = "account-ext")]
                extension,
            },
            rest,
        )
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compress for Account {
    type Compressed = alloc::vec::Vec<u8>;

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        let _ = reth_codecs::Compact::to_compact(self, buf);
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Decompress for Account {
    fn decompress(value: &[u8]) -> Result<Self, reth_codecs::DecompressError> {
        validate_compact_account(value).map_err(reth_codecs::DecompressError::new)?;
        // All lengths used by the existing infallible Compact decoder are checked above.
        // Keeping that decoder retains the exact semantics of valid legacy records.
        Ok(reth_codecs::Compact::from_compact(value, value.len()).0)
    }
}

#[cfg(feature = "reth-codec")]
#[derive(Debug, thiserror::Error)]
#[error("invalid compact account: {0}")]
struct InvalidCompactAccount(&'static str);

#[cfg(feature = "reth-codec")]
fn validate_compact_account(value: &[u8]) -> Result<(), InvalidCompactAccount> {
    let flags_len = LegacyAccount::bitflag_encoded_bytes();
    if value.len() < flags_len {
        return Err(InvalidCompactAccount("truncated flags"));
    }
    let (flags, _) = LegacyAccountFlags::from(value);
    let nonce_len = usize::from(flags.nonce_len());
    let balance_len = usize::from(flags.balance_len());
    if nonce_len > 8 || balance_len > 32 {
        return Err(InvalidCompactAccount("invalid integer length"));
    }
    let legacy_len =
        flags_len + nonce_len + balance_len + usize::from(flags.bytecode_hash_len()) * 32;
    let extension = value
        .get(legacy_len..)
        .ok_or(InvalidCompactAccount("truncated legacy fields"))?;
    if extension.is_empty() {
        return Ok(());
    }
    #[cfg(not(feature = "account-ext"))]
    return Err(InvalidCompactAccount(
        "account extensions require account-ext",
    ));
    #[cfg(feature = "account-ext")]
    {
        if extension.len() < 2 {
            return Err(InvalidCompactAccount("truncated opaque length"));
        }
        let opaque_len = usize::from(u16::from_be_bytes([extension[0], extension[1]]));
        let trailer = extension
            .get(2 + opaque_len..)
            .ok_or(InvalidCompactAccount("truncated opaque fields"))?;
        if trailer.is_empty() {
            return Ok(());
        }
        if trailer.len() < 3 || trailer[0] != 1 {
            return Err(InvalidCompactAccount("invalid typed extension version"));
        }
        let metadata_len = usize::from(u16::from_be_bytes([trailer[1], trailer[2]]));
        if metadata_len == 0 || metadata_len > 4096 || trailer.len() != 3 + metadata_len {
            return Err(InvalidCompactAccount("invalid code metadata length"));
        }
        Ok(())
    }
}

impl Account {
    /// Whether the account has bytecode.
    #[inline]
    pub const fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }

    /// After `SpuriousDragon` empty account is defined as account with nonce == 0 && balance == 0
    /// && bytecode = None (or hash is [`KECCAK_EMPTY`]).
    #[inline]
    pub fn is_empty(&self) -> bool {
        let empty = self.nonce == 0
            && self.balance.is_zero()
            && self.bytecode_hash.is_none_or(|hash| hash == KECCAK_EMPTY);
        #[cfg(feature = "account-ext")]
        let empty = empty && self.extension.is_empty();
        empty
    }

    /// Returns an account bytecode's hash.
    /// In case of no bytecode, returns [`KECCAK_EMPTY`].
    #[inline]
    pub fn get_bytecode_hash(&self) -> B256 {
        self.bytecode_hash.unwrap_or(KECCAK_EMPTY)
    }

    /// Converts the account into a trie account with the given storage root.
    #[inline]
    pub fn into_trie_account(self, storage_root: B256) -> TrieAccount {
        let Self {
            nonce,
            balance,
            bytecode_hash,
            #[cfg(feature = "account-ext")]
            extension,
        } = self;
        TrieAccount {
            nonce,
            balance,
            storage_root,
            code_hash: bytecode_hash.unwrap_or(KECCAK_EMPTY),
            #[cfg(feature = "account-ext")]
            extension,
        }
    }

    /// Extracts the account information from a [`revm_state::Account`]
    pub fn from_revm_account(revm_account: &revm_state::Account) -> Self {
        Self {
            balance: revm_account.info.balance,
            nonce: revm_account.info.nonce,
            bytecode_hash: if revm_account.info.code_hash == revm_primitives::KECCAK_EMPTY {
                None
            } else {
                Some(revm_account.info.code_hash)
            },
            #[cfg(feature = "account-ext")]
            extension: alloy_trie::AccountExtension::from_shared(
                revm_account.info.extension.clone().into_shared(),
            ),
        }
    }
}

#[cfg(feature = "eip7928")]
impl Account {
    /// Applies the post-block account fields an EIP-7928 block access list entry recorded on top
    /// of this account.
    ///
    /// A block access list only carries the fields a block changed, so `self` must be the account
    /// as it was before the block; fields the entry leaves out keep their current values. Use
    /// [`Account::default`] as the base when the account did not exist.
    ///
    /// An account left without code ends up with [`KECCAK_EMPTY`] rather than `None`. Both encode
    /// the same trie leaf, see [`Self::into_trie_account`].
    pub fn apply_bal_info(&mut self, info: alloy_eip7928::BalAccountInfo) {
        if let Some(balance) = info.balance {
            self.balance = balance;
        }
        if let Some(nonce) = info.nonce {
            self.nonce = nonce;
        }
        self.bytecode_hash = info.code_hash.or(self.bytecode_hash).or(Some(KECCAK_EMPTY));
    }
}

impl From<revm_state::Account> for Account {
    #[inline]
    fn from(value: revm_state::Account) -> Self {
        Self::from(value.info)
    }
}

impl From<TrieAccount> for Account {
    fn from(value: TrieAccount) -> Self {
        Self {
            balance: value.balance,
            nonce: value.nonce,
            bytecode_hash: (value.code_hash != KECCAK_EMPTY).then_some(value.code_hash),
            #[cfg(feature = "account-ext")]
            extension: value.extension,
        }
    }
}

impl InMemorySize for Account {
    #[inline]
    fn size(&self) -> usize {
        let size = size_of::<u64>() + size_of::<U256>() + size_of::<Option<B256>>();
        #[cfg(feature = "account-ext")]
        let size = size
            + size_of::<alloy_trie::AccountExtension>()
            + if self.extension.is_empty() {
                0
            } else {
                2 * size_of::<usize>()
                    + self.extension.len()
                    + self
                        .extension
                        .code_metadata()
                        .map_or(0, |code| 4 + code.len())
            };
        size
    }
}

/// Bytecode for an account.
///
/// A wrapper around [`revm::primitives::Bytecode`][RevmBytecode] with encoding/decoding support.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deref)]
pub struct Bytecode(pub RevmBytecode);

impl Bytecode {
    /// Create new bytecode from raw bytes.
    ///
    /// No analysis will be performed.
    ///
    /// # Panics
    ///
    /// Panics if bytecode is EOF and has incorrect format.
    #[inline]
    pub fn new_raw(bytes: Bytes) -> Self {
        Self(RevmBytecode::new_raw(bytes))
    }

    /// Creates a new raw [`revm_bytecode::Bytecode`].
    ///
    /// Returns an error on incorrect Bytecode format.
    #[inline]
    pub fn new_raw_checked(bytecode: Bytes) -> Result<Self, BytecodeDecodeError> {
        RevmBytecode::new_raw_checked(bytecode).map(Self)
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for Bytecode {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        use compact_ids::{EIP7702_BYTECODE_ID, LEGACY_ANALYZED_BYTECODE_ID};

        let bytecode = self.0.bytes_ref();
        buf.put_u32(bytecode.len() as u32);
        buf.put_slice(bytecode.as_ref());
        let len = if self.0.is_legacy() {
            // [`REMOVED_BYTECODE_ID`] has been removed.
            if let Some(jump_table) = self.0.legacy_jump_table() {
                buf.put_u8(LEGACY_ANALYZED_BYTECODE_ID);
                buf.put_u64(self.0.len() as u64);
                let map = jump_table.as_slice();
                buf.put_slice(map);
                1 + 8 + map.len()
            } else {
                unreachable!("legacy bytecode must contain a jump table")
            }
        } else {
            buf.put_u8(EIP7702_BYTECODE_ID);
            1
        };
        len + bytecode.len() + 4
    }

    // # Panics
    //
    // A panic will be triggered if a bytecode variant of 1 or greater than 2 is passed from the
    // database.
    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        use byteorder::ReadBytesExt;
        use bytes::Buf;

        use compact_ids::*;

        let len = buf
            .read_u32::<byteorder::BigEndian>()
            .expect("could not read bytecode length") as usize;
        let bytes = Bytes::from(buf.copy_to_bytes(len));
        let variant = buf.read_u8().expect("could not read bytecode variant");
        let decoded = match variant {
            LEGACY_RAW_BYTECODE_ID => Self(RevmBytecode::new_raw(bytes)),
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
                // SAFETY: jump table is constructed from the persisted bitvec and the bytecode
                // length matches; this is the inverse of the original `to_compact` encoding.
                Self(unsafe {
                    RevmBytecode::new_analyzed(
                        bytes,
                        original_len,
                        revm_bytecode::JumpTable::from_slice(buf, jump_table_len),
                    )
                })
            }
            EIP7702_BYTECODE_ID => {
                // EIP-7702 bytecode objects will be decoded from the raw bytecode
                Self(RevmBytecode::new_raw(bytes))
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
            #[cfg(feature = "account-ext")]
            extension: value.extension.clone(),
        }
    }
}

impl From<AccountInfo> for Account {
    fn from(revm_acc: AccountInfo) -> Self {
        Self {
            balance: revm_acc.balance,
            nonce: revm_acc.nonce,
            bytecode_hash: (!revm_acc.is_empty_code_hash()).then_some(revm_acc.code_hash),
            #[cfg(feature = "account-ext")]
            extension: alloy_trie::AccountExtension::from_shared(revm_acc.extension.into_shared()),
        }
    }
}

impl From<&AccountInfo> for Account {
    fn from(revm_acc: &AccountInfo) -> Self {
        Self {
            balance: revm_acc.balance,
            nonce: revm_acc.nonce,
            bytecode_hash: (!revm_acc.is_empty_code_hash()).then_some(revm_acc.code_hash),
            #[cfg(feature = "account-ext")]
            extension: alloy_trie::AccountExtension::from_shared(
                revm_acc.extension.clone().into_shared(),
            ),
        }
    }
}

impl From<Account> for AccountInfo {
    fn from(reth_acc: Account) -> Self {
        Self {
            balance: reth_acc.balance,
            nonce: reth_acc.nonce,
            code_hash: reth_acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
            account_id: None,
            #[cfg(feature = "account-ext")]
            extension: revm_state::AccountExtension::from_shared(reth_acc.extension.into_shared()),
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::*;

    #[test]
    fn empty_extension_is_not_serialized() {
        let json = serde_json::to_value(Account::default()).unwrap();
        assert!(!json.as_object().unwrap().contains_key("extension"));
    }

    #[test]
    fn account_messagepack_compatibility() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct LegacyAccount {
            nonce: u64,
            balance: U256,
            bytecode_hash: Option<B256>,
        }
        let account = Account {
            nonce: 7,
            balance: U256::from(42),
            ..Default::default()
        };
        let legacy = LegacyAccount {
            nonce: account.nonce,
            balance: account.balance,
            bytecode_hash: account.bytecode_hash,
        };
        let encoded = rmp_serde::to_vec(&account).unwrap();
        assert_eq!(encoded, rmp_serde::to_vec(&legacy).unwrap());
        assert_eq!(rmp_serde::from_slice::<Account>(&encoded).unwrap(), account);
        let decoded: LegacyAccount = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(rmp_serde::to_vec(&decoded).unwrap(), encoded);

        let accounts = alloc::vec![
            account,
            Account {
                nonce: 9,
                #[cfg(feature = "account-ext")]
                extension: alloy_trie::AccountExtension::copy_from_slice(&[0x82, 0xaa]),
                ..Default::default()
            },
            Account::default(),
        ];
        let record = (accounts, 99u64);
        let encoded = rmp_serde::to_vec(&record).unwrap();
        let decoded: (alloc::vec::Vec<Account>, u64) = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(decoded, record);
    }
}

#[cfg(all(test, feature = "std", feature = "reth-codec"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use alloy_primitives::{B256, U256, hex_literal::hex};
    use reth_codecs::Compact;
    use revm_bytecode::JumpTable;

    #[test]
    fn empty_extension_preserves_compact() {
        let account = Account::default();
        let mut actual = Vec::new();
        let len = account.to_compact(&mut actual);
        let mut legacy = Vec::new();
        LegacyAccount {
            nonce: account.nonce,
            balance: account.balance,
            bytecode_hash: account.bytecode_hash,
        }
        .to_compact(&mut legacy);
        assert_eq!(actual, legacy);
        assert_eq!(Account::from_compact(&actual, len).0, account);
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn extension_roundtrips_without_copying_shared_payload() {
        let account = Account {
            extension: alloy_trie::AccountExtension::copy_from_slice(&[0x01, 0x02]),
            ..Default::default()
        };
        assert!(!account.is_empty());
        let pointer = account.extension.as_ptr();
        let revm = AccountInfo::from(account.clone());
        assert_eq!(revm.extension.as_ptr(), pointer);
        let restored = Account::from(revm);
        assert_eq!(restored.extension.as_ptr(), pointer);
        assert_eq!(restored, account);
        let trie = account
            .clone()
            .into_trie_account(alloy_trie::EMPTY_ROOT_HASH);
        assert_eq!(trie.extension.as_ptr(), pointer);
        assert_eq!(Account::from(trie), account);

        let mut compact = Vec::new();
        let len = account.to_compact(&mut compact);
        assert!(compact.ends_with(&[0, 2, 0x01, 0x02]));
        compact.extend_from_slice(&[99, 100]);
        let (restored, rest) = Account::from_compact(&compact, len);
        assert_eq!(restored, account);
        assert_eq!(rest, &[99, 100]);
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn compact_extension_length_boundaries() {
        for len in [1, 256, 2048, usize::from(u16::MAX)] {
            let account = Account {
                extension: alloy_trie::AccountExtension::from(alloc::vec![0x82; len]),
                ..Default::default()
            };
            let mut compact = Vec::new();
            let encoded_len = account.to_compact(&mut compact);
            assert_eq!(
                &compact[compact.len() - len - 2..compact.len() - len],
                &(len as u16).to_be_bytes()
            );
            assert_eq!(Account::from_compact(&compact, encoded_len).0, account);
        }
    }

    #[test]
    fn test_empty_account() {
        let mut acc = Account::default();
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

        let mut buf = vec![];
        // SAFETY: hand-crafted analyzed bytecode used purely for round-trip testing.
        let bytecode = Bytecode(unsafe {
            RevmBytecode::new_analyzed(
                Bytes::from(&hex!("ff00")),
                2,
                JumpTable::from_slice(&[0], 2),
            )
        });
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 16);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_account_has_bytecode() {
        // Account with no bytecode (None)
        let acc_no_bytecode: Account = Account {
            nonce: 1,
            balance: U256::from(1000),
            ..Default::default()
        };
        assert!(
            !acc_no_bytecode.has_bytecode(),
            "Account should not have bytecode"
        );

        // Account with bytecode hash set to KECCAK_EMPTY (should have bytecode)
        let acc_empty_bytecode: Account = Account {
            bytecode_hash: Some(KECCAK_EMPTY),
            ..Default::default()
        };
        assert!(
            acc_empty_bytecode.has_bytecode(),
            "Account should have bytecode"
        );

        // Account with a non-empty bytecode hash
        let acc_with_bytecode: Account = Account {
            bytecode_hash: Some(B256::from_slice(&[0x11u8; 32])),
            ..Default::default()
        };
        assert!(
            acc_with_bytecode.has_bytecode(),
            "Account should have bytecode"
        );
    }

    #[test]
    fn test_account_get_bytecode_hash() {
        // Account with no bytecode (should return KECCAK_EMPTY)
        let acc_no_bytecode: Account = Default::default();
        assert_eq!(
            acc_no_bytecode.get_bytecode_hash(),
            KECCAK_EMPTY,
            "Should return KECCAK_EMPTY"
        );

        // Account with bytecode hash set to KECCAK_EMPTY
        let acc_empty_bytecode: Account = Account {
            bytecode_hash: Some(KECCAK_EMPTY),
            ..Default::default()
        };
        assert_eq!(
            acc_empty_bytecode.get_bytecode_hash(),
            KECCAK_EMPTY,
            "Should return KECCAK_EMPTY"
        );

        // Account with a valid bytecode hash
        let bytecode_hash = B256::from_slice(&[0x11u8; 32]);
        let acc_with_bytecode: Account = Account {
            bytecode_hash: Some(bytecode_hash),
            ..Default::default()
        };
        assert_eq!(
            acc_with_bytecode.get_bytecode_hash(),
            bytecode_hash,
            "Should return the bytecode hash"
        );
    }
}

#[cfg(all(test, feature = "eip7928"))]
mod bal_tests {
    use super::*;
    use alloy_eip7928::BalAccountInfo;

    fn account() -> Account {
        Account {
            nonce: 4,
            balance: U256::from(9),
            bytecode_hash: Some(B256::repeat_byte(1)),
            #[cfg(feature = "account-ext")]
            extension: Default::default(),
        }
    }

    #[test]
    fn untouched_fields_keep_their_values() {
        let mut acc = account();
        acc.apply_bal_info(BalAccountInfo {
            balance: Some(U256::from(99)),
            ..Default::default()
        });

        assert_eq!(acc.balance, U256::from(99));
        assert_eq!(acc.nonce, 4);
        assert_eq!(acc.bytecode_hash, Some(B256::repeat_byte(1)));
    }

    #[test]
    fn a_new_account_defaults_its_untouched_fields() {
        let mut acc = Account::default();
        acc.apply_bal_info(BalAccountInfo {
            balance: Some(U256::from(1)),
            ..Default::default()
        });

        assert_eq!(acc.nonce, 0);
        assert_eq!(acc.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn changed_code_replaces_the_previous_hash() {
        let mut acc = account();
        acc.apply_bal_info(BalAccountInfo {
            code_hash: Some(KECCAK_EMPTY),
            ..Default::default()
        });

        assert_eq!(acc.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn an_account_funded_then_emptied_is_left_empty() {
        let mut acc = Account::default();
        acc.apply_bal_info(BalAccountInfo {
            balance: Some(U256::ZERO),
            ..Default::default()
        });

        assert!(acc.is_empty());
    }
}

#[cfg(all(test, feature = "account-ext", feature = "reth-codec"))]
mod typed_extension_tests {
    use super::*;
    use reth_codecs::Compact;

    #[test]
    fn typed_extension_survives_compact_revm_and_trie() {
        for opaque_len in [0, 1, 255, 65535] {
            let extension =
                alloy_trie::AccountExtension::copy_from_slice(&alloc::vec![0xff; opaque_len])
                    .with_code_metadata(&[2; 21]);
            let account = Account::new(7, U256::from(11), Some(B256::repeat_byte(3)))
                .with_extension(extension);
            let mut bytes = alloc::vec::Vec::new();
            let len = account.to_compact(&mut bytes);
            assert_eq!(len, bytes.len());
            let (decoded, rest) = Account::from_compact(&bytes, len);
            assert!(rest.is_empty());
            assert_eq!(decoded, account);
            let revm: AccountInfo = account.clone().into();
            assert_eq!(revm.extension.code_metadata(), Some(&[2; 21][..]));
            assert_eq!(Account::from(revm), account);
            assert_eq!(
                account.clone().into_trie_account(B256::ZERO).extension,
                account.extension
            );
        }
    }
}

#[cfg(all(test, feature = "account-ext", feature = "reth-codec"))]
mod checked_account_decompression_tests {
    use super::*;
    use reth_codecs::{Compress, Decompress};

    #[test]
    fn malformed_typed_records_are_errors_at_database_boundary() {
        let legacy = Account::new(7, U256::from(11), Some(B256::repeat_byte(3)));
        let base_len = legacy.clone().compress().len();
        let account = legacy.with_extension(
            alloy_trie::AccountExtension::copy_from_slice(&[0xab, 0xcd])
                .with_code_metadata(&[2; 21]),
        );
        let encoded = account.clone().compress();
        assert_eq!(Account::decompress(&encoded).unwrap(), account);
        let trailer = base_len + 2 + 2;
        for index in [trailer, trailer + 1, trailer + 2] {
            let mut corrupt = encoded.clone();
            corrupt[index] = 0xff;
            assert!(Account::decompress(&corrupt).is_err());
        }
        assert!(Account::decompress(&encoded[..encoded.len() - 1]).is_err());
        assert!(Account::decompress(&encoded[..base_len - 1]).is_err());
        assert!(Account::decompress(&[]).is_err());
        assert!(Account::decompress(&[0xff, 0xff]).is_err());
        let mut extra = encoded;
        extra.push(0);
        assert!(Account::decompress(&extra).is_err());
    }

    #[test]
    fn valid_legacy_records_keep_their_exact_encoding() {
        for opaque in [alloc::vec![], alloc::vec![1; 20], alloc::vec![0xff; 65535]] {
            let original = Account::new(1, U256::MAX, None).with_extension(opaque);
            let encoded = original.clone().compress();
            let decoded = Account::decompress(&encoded).unwrap();
            assert_eq!(decoded, original);
            assert_eq!(decoded.compress(), encoded);
        }
    }
}
