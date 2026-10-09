#[cfg(feature = "account-ext")]
use crate::AccountExtension;
use bytecode::Bytecode;
use core::{
    cmp::Ordering,
    hash::{Hash, Hasher},
};
use primitives::{B256, KECCAK_EMPTY, U256};

use nonmax::NonMaxU32;

/// Account ID is a custom type that wraps a `NonMaxU32`
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountId(NonMaxU32);

impl AccountId {
    /// Creates a new AccountId.
    ///
    /// Returns `None` if the value does not fit in the internal representation.
    #[inline]
    pub fn new(id: usize) -> Option<Self> {
        let id = u32::try_from(id).ok()?;
        NonMaxU32::new(id).map(Self)
    }

    /// Gets the account ID as a usize.
    #[inline]
    pub const fn get(self) -> usize {
        self.0.get() as usize
    }
}

/// Account information that contains balance, nonce, code hash and code
///
/// Code is set as optional.
///
/// The opt-in `account-ext` feature adds a shared, ThinArc-backed extension payload.
/// Without it, the account layout and serialization have no extension field.
#[derive(Clone, Debug, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountInfo {
    /// Account balance.
    pub balance: U256,
    /// Account nonce.
    pub nonce: u64,
    /// Hash of the raw bytes in `code`, or [`KECCAK_EMPTY`].
    pub code_hash: B256,
    /// Used as a hint to optimize the access to the storage of account.
    ///
    /// It is set when account is loaded from the database, and if it is `Some` it will called
    /// by journal to ask database the storage with this account_id (It will still send the address to the database).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub account_id: Option<AccountId>,
    /// [`Bytecode`] data associated with this account.
    ///
    /// If [`None`], `code_hash` will be used to fetch it from the database, if code needs to be
    /// loaded from inside `revm`.
    ///
    /// By default, this is `Some(Bytecode::default())`.
    pub code: Option<Bytecode>,
    /// Chain-specific account data carried through execution and state transitions.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "AccountExtension::is_empty")
    )]
    #[cfg(feature = "account-ext")]
    pub extension: AccountExtension,
}

impl Default for AccountInfo {
    #[inline]
    fn default() -> Self {
        Self {
            balance: U256::ZERO,
            code_hash: KECCAK_EMPTY,
            account_id: None,
            nonce: 0,
            code: Some(Bytecode::default()),
            #[cfg(feature = "account-ext")]
            extension: AccountExtension::new(),
        }
    }
}

impl PartialEq for AccountInfo {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        let equal = self.balance == other.balance
            && self.nonce == other.nonce
            && self.code_hash == other.code_hash;
        #[cfg(feature = "account-ext")]
        let equal = equal && self.extension == other.extension;
        equal
    }
}

impl Hash for AccountInfo {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.balance.hash(state);
        self.nonce.hash(state);
        self.code_hash.hash(state);
    }
}

impl PartialOrd for AccountInfo {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AccountInfo {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        let order = self
            .balance
            .cmp(&other.balance)
            .then_with(|| self.nonce.cmp(&other.nonce))
            .then_with(|| self.code_hash.cmp(&other.code_hash));
        #[cfg(feature = "account-ext")]
        let order = order.then_with(|| self.extension.cmp(&other.extension));
        order
    }
}

impl AccountInfo {
    /// Creates a new [`AccountInfo`] with the given fields.
    #[inline]
    pub const fn new(balance: U256, nonce: u64, code_hash: B256, code: Bytecode) -> Self {
        Self {
            balance,
            nonce,
            code: Some(code),
            code_hash,
            account_id: None,
            #[cfg(feature = "account-ext")]
            extension: AccountExtension::new(),
        }
    }

    /// Creates a new [`AccountInfo`] with the given code.
    ///
    /// # Note
    ///
    /// As code hash is calculated with [`Bytecode::hash_slow`] there will be performance penalty if used frequently.
    #[inline]
    pub fn with_code(self, code: Bytecode) -> Self {
        Self {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..self
        }
    }

    /// Creates a new [`AccountInfo`] with the given code hash.
    ///
    /// # Note
    ///
    /// Resets code to `None`. Not guaranteed to maintain invariant `code` and `code_hash`. See
    /// also [Self::with_code_and_hash].
    #[inline]
    pub fn with_code_hash(self, code_hash: B256) -> Self {
        Self {
            code_hash,
            code: None,
            ..self
        }
    }

    /// Creates a new [`AccountInfo`] with the given code and code hash.
    ///
    /// # Note
    ///
    /// In debug mode panics if [`Bytecode::hash_slow`] called on `code` is not equivalent to
    /// `code_hash`. See also [`Self::with_code`].
    #[inline]
    pub fn with_code_and_hash(self, code: Bytecode, code_hash: B256) -> Self {
        debug_assert_eq!(code.hash_slow(), code_hash);
        Self {
            code_hash,
            code: Some(code),
            ..self
        }
    }

    /// Creates a new [`AccountInfo`] with the given balance.
    #[inline]
    pub const fn with_balance(mut self, balance: U256) -> Self {
        self.balance = balance;
        self
    }

    /// Creates a new [`AccountInfo`] with the given nonce.
    #[inline]
    pub const fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    /// Sets the [`AccountInfo`] `balance`.
    #[inline]
    pub const fn set_balance(&mut self, balance: U256) -> &mut Self {
        self.balance = balance;
        self
    }

    /// Sets the [`AccountInfo`] `nonce`.
    #[inline]
    pub const fn set_nonce(&mut self, nonce: u64) -> &mut Self {
        self.nonce = nonce;
        self
    }

    /// Sets the [`AccountInfo`] `code_hash` and clears any cached bytecode.
    ///
    /// # Note
    ///
    /// Calling this after `set_code(...)` will remove the bytecode you just set.
    /// If you intend to mutate the code, use only `set_code`.
    #[inline]
    pub fn set_code_hash(&mut self, code_hash: B256) -> &mut Self {
        self.code = None;
        self.code_hash = code_hash;
        self
    }

    /// Replaces the [`AccountInfo`] bytecode and recalculates `code_hash`.
    ///
    /// # Note
    ///
    /// As code hash is calculated with [`Bytecode::hash_slow`] there will be performance penalty if used frequently.
    #[inline]
    pub fn set_code(&mut self, code: Bytecode) -> &mut Self {
        self.code_hash = code.hash_slow();
        self.code = Some(code);
        self
    }
    /// Sets the bytecode and its hash, returning the previous code and hash.
    ///
    /// # Note
    ///
    /// It is on the caller's responsibility to ensure that the bytecode hash is correct.
    pub const fn set_code_and_hash(
        &mut self,
        code: Bytecode,
        code_hash: B256,
    ) -> (B256, Option<Bytecode>) {
        let previous_hash = core::mem::replace(&mut self.code_hash, code_hash);
        let previous_code = self.code.replace(code);
        (previous_hash, previous_code)
    }
    /// Returns a copy of this account with the [`Bytecode`] removed.
    ///
    /// This is useful when creating journals or snapshots of the state, where it is
    /// desirable to store the code blobs elsewhere.
    ///
    /// ## Note
    ///
    /// This is distinct from [`without_code`][Self::without_code] in that it returns
    /// a new [`AccountInfo`] instance with the code removed.
    ///
    /// [`without_code`][Self::without_code] will modify and return the same instance.
    #[inline]
    #[cfg(feature = "account-ext")]
    pub fn copy_without_code(&self) -> Self {
        Self {
            balance: self.balance,
            nonce: self.nonce,
            code_hash: self.code_hash,
            account_id: self.account_id,
            code: None,
            extension: self.extension.clone(),
        }
    }

    /// Returns a copy of this account with the bytecode removed.
    #[inline]
    #[cfg(not(feature = "account-ext"))]
    pub const fn copy_without_code(&self) -> Self {
        Self {
            balance: self.balance,
            nonce: self.nonce,
            code_hash: self.code_hash,
            account_id: self.account_id,
            code: None,
        }
    }

    /// Strips the [`Bytecode`] from this account and drop it.
    ///
    /// This is useful when creating journals or snapshots of the state, where it is
    /// desirable to store the code blobs elsewhere.
    ///
    /// ## Note
    ///
    /// This is distinct from [`copy_without_code`][Self::copy_without_code] in that it
    /// modifies the account in place.
    ///
    /// [`copy_without_code`][Self::copy_without_code]
    /// will copy the non-code fields and return a new [`AccountInfo`] instance.
    #[inline]
    pub fn without_code(mut self) -> Self {
        self.take_bytecode();
        self
    }

    /// Returns if an account is empty.
    ///
    /// An account is empty if the following conditions are met.
    /// - code hash is zero or set to the Keccak256 hash of the empty string `""`
    /// - balance is zero
    /// - nonce is zero
    #[inline]
    pub fn is_empty(&self) -> bool {
        let empty = self.is_code_hash_empty_or_zero() && self.balance.is_zero() && self.nonce == 0;
        #[cfg(feature = "account-ext")]
        let empty = empty && self.extension.is_empty();
        empty
    }

    /// Optimization hint.
    #[inline]
    pub(crate) fn is_default(&self) -> bool {
        self.is_empty() && self.code.as_ref().is_some_and(Bytecode::is_default)
    }

    /// Returns `true` if the account is not empty.
    #[inline]
    pub fn exists(&self) -> bool {
        !self.is_empty()
    }

    /// Returns `true` if account has no nonce and code.
    #[inline]
    pub fn has_no_code_and_nonce(&self) -> bool {
        self.is_empty_code_hash() && self.nonce == 0
    }

    /// Returns bytecode hash associated with this account.
    ///
    /// If account does not have code, it returns `KECCAK_EMPTY` hash.
    #[inline]
    pub const fn code_hash(&self) -> B256 {
        self.code_hash
    }

    /// Returns this account with chain-specific extension data.
    #[inline]
    #[cfg(feature = "account-ext")]
    pub fn with_extension(mut self, extension: impl Into<AccountExtension>) -> Self {
        self.extension = extension.into();
        self
    }

    /// Replaces the chain-specific extension data.
    #[inline]
    #[cfg(feature = "account-ext")]
    pub const fn set_extension(&mut self, extension: AccountExtension) -> AccountExtension {
        core::mem::replace(&mut self.extension, extension)
    }

    /// Returns true if the code hash is the Keccak256 hash of the empty string `""`.
    #[inline]
    pub fn is_empty_code_hash(&self) -> bool {
        self.code_hash == KECCAK_EMPTY
    }

    /// Returns true if the code hash is the Keccak256 hash of the empty string `""` or is zero.
    #[inline]
    pub fn is_code_hash_empty_or_zero(&self) -> bool {
        self.is_empty_code_hash() || self.code_hash.is_zero()
    }

    /// Takes bytecode from account.
    ///
    /// Code will be set to [None].
    #[inline]
    pub const fn take_bytecode(&mut self) -> Option<Bytecode> {
        self.code.take()
    }

    /// Initializes an [`AccountInfo`] with the given balance, setting all other fields to their
    /// default values.
    #[inline]
    pub fn from_balance(balance: U256) -> Self {
        AccountInfo {
            balance,
            ..Default::default()
        }
    }

    /// Initializes an [`AccountInfo`] with the given bytecode, setting its balance to zero, its
    /// nonce to `1`, and calculating the code hash from the given bytecode.
    #[inline]
    pub fn from_bytecode(bytecode: Bytecode) -> Self {
        let hash = bytecode.hash_slow();

        AccountInfo {
            balance: U256::ZERO,
            nonce: 1,
            code: Some(bytecode),
            code_hash: hash,
            account_id: None,
            #[cfg(feature = "account-ext")]
            extension: AccountExtension::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn account_info_inline_size() {
        assert_eq!(
            size_of::<AccountInfo>(),
            if cfg!(feature = "account-ext") {
                96
            } else {
                88
            }
        );
    }

    #[test]
    #[cfg(feature = "serde")]
    fn empty_extension_preserves_legacy_serde() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct LegacyAccountInfo {
            balance: U256,
            nonce: u64,
            code_hash: B256,
            code: Option<Bytecode>,
        }
        let account = AccountInfo::new(U256::from(42), 7, KECCAK_EMPTY, Bytecode::default());
        let legacy = LegacyAccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.code_hash,
            code: account.code.clone(),
        };
        let json = serde_json::to_vec(&legacy).unwrap();
        assert_eq!(serde_json::to_vec(&account).unwrap(), json);
        assert_eq!(
            serde_json::from_slice::<AccountInfo>(&json).unwrap(),
            account
        );
        let encoded = rmp_serde::to_vec(&account).unwrap();
        assert_eq!(encoded, rmp_serde::to_vec(&legacy).unwrap());
        assert_eq!(
            rmp_serde::from_slice::<AccountInfo>(&encoded).unwrap(),
            account
        );
        let decoded: LegacyAccountInfo = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(rmp_serde::to_vec(&decoded).unwrap(), encoded);
        let binary = postcard::to_allocvec(&legacy).unwrap();
        assert_eq!(postcard::to_allocvec(&account).unwrap(), binary);
        #[cfg(not(feature = "account-ext"))]
        assert_eq!(
            postcard::from_bytes::<AccountInfo>(&binary).unwrap(),
            account
        );
    }

    #[test]
    #[cfg(all(feature = "serde", feature = "account-ext"))]
    fn account_info_messagepack_roundtrip() {
        let accounts = vec![
            AccountInfo::default(),
            AccountInfo::default().with_extension(vec![0x82, 0xaa]),
            AccountInfo::default(),
        ];
        let record = (accounts, 99u64);
        let encoded = rmp_serde::to_vec(&record).unwrap();
        let decoded: (Vec<AccountInfo>, u64) = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn test_account_info_trait_consistency() {
        let bytecode = Bytecode::default();
        let account1 = AccountInfo {
            code: Some(bytecode),
            ..AccountInfo::default()
        };

        let account2 = AccountInfo::default();

        assert_eq!(account1, account2, "Accounts should be equal ignoring code");

        assert_eq!(
            account1.cmp(&account2),
            Ordering::Equal,
            "Ordering should be equal after ignoring code in Ord"
        );

        #[expect(clippy::mutable_key_type)] // Not observable
        let mut set = BTreeSet::new();
        assert!(set.insert(account1.clone()), "Inserted account1");
        assert!(
            !set.insert(account2.clone()),
            "account2 not inserted (treated as duplicate)"
        );

        assert_eq!(set.len(), 1, "Set should have only one unique account");
        assert!(set.contains(&account1), "Set contains account1");
        assert!(
            set.contains(&account2),
            "Set contains account2 (since equal)"
        );

        let mut accounts = [account2, account1];
        accounts.sort();
        assert_eq!(accounts[0], accounts[1], "Sorted vec treats them as equal");
    }

    #[test]
    fn is_default() {
        assert!(AccountInfo::default().is_default())
    }

    #[test]
    #[cfg(feature = "serde")]
    fn is_default_after_serde() {
        let info = AccountInfo::default();
        let json = serde_json::to_string(&info).unwrap();
        let deser: AccountInfo = serde_json::from_str(&json).unwrap();
        assert!(deser.is_default());
    }

    #[test]
    #[cfg(feature = "account-ext")]
    fn extension_participates_in_account_identity() {
        let base = AccountInfo::default();
        let extended = base
            .clone()
            .with_extension(AccountExtension::copy_from_slice(b"extension"));

        assert_ne!(base, extended);
        assert_ne!(base.cmp(&extended), Ordering::Equal);
        assert!(base.is_empty());
        assert!(!extended.is_empty());
    }

    #[test]
    #[cfg(feature = "serde")]
    #[cfg(feature = "account-ext")]
    fn missing_extension_decodes_as_empty() {
        let mut json = serde_json::to_value(AccountInfo::default()).unwrap();
        json.as_object_mut().unwrap().remove("extension");
        let decoded: AccountInfo = serde_json::from_value(json).unwrap();
        assert!(decoded.extension.is_empty());
    }
}
