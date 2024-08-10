use crate::Compact;
use alloy_genesis::GenesisAccount as AlloyGenesisAccount;
use alloy_primitives::{Bytes, B256, U256};
use reth_codecs_derive::reth_codec;
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// GenesisAccount acts as bridge which simplifies Compact implementation for AlloyGenesisAccount.
///
/// Notice: Make sure this struct is 1:1 with `alloy_genesis::GenesisAccount`
#[reth_codec(no_arbitrary)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct GenesisAccountRef<'a> {
    /// The nonce of the account at genesis.
    nonce: Option<u64>,
    /// The balance of the account at genesis.
    balance: &'a U256,
    /// The account's bytecode at genesis.
    code: Option<&'a Bytes>,
    /// The account's storage at genesis.
    storage: Option<StorageEntries>,
    /// The account's private key. Should only be used for testing.
    private_key: Option<&'a B256>,
}

#[reth_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct GenesisAccount {
    /// The nonce of the account at genesis.
    nonce: Option<u64>,
    /// The balance of the account at genesis.
    balance: U256,
    /// The account's bytecode at genesis.
    code: Option<Bytes>,
    /// The account's storage at genesis.
    storage: Option<StorageEntries>,
    /// The account's private key. Should only be used for testing.
    private_key: Option<B256>,
}

#[reth_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct StorageEntries {
    entries: Vec<StorageEntry>,
}

#[reth_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct StorageEntry {
    key: B256,
    value: B256,
}

impl Compact for AlloyGenesisAccount {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let account = GenesisAccountRef {
            nonce: self.nonce,
            balance: &self.balance,
            code: self.code.as_ref(),
            storage: self.storage.as_ref().map(|s| StorageEntries {
                entries: s
                    .iter()
                    .map(|(key, value)| StorageEntry { key: *key, value: *value })
                    .collect(),
            }),
            private_key: self.private_key.as_ref(),
        };
        account.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (account, _) = GenesisAccount::from_compact(buf, len);
        let alloy_account = Self {
            nonce: account.nonce,
            balance: account.balance,
            code: account.code,
            storage: account
                .storage
                .map(|s| s.entries.into_iter().map(|entry| (entry.key, entry.value)).collect()),
            private_key: account.private_key,
        };
        (alloy_account, buf)
    }
}
