use crate::Compact;
use alloy_genesis::GenesisAccount as AlloyGenesisAccount;
use alloy_primitives::{Bytes, B256, U256};
use reth_codecs_derive::main_codec;

/// GenesisAccount acts as bridge which simplifies Compact implementation for AlloyGenesisAccount.
///
/// Notice: Make sure this struct is 1:1 with `alloy_genesis::GenesisAccount`
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct StorageEntries {
    entries: Vec<StorageEntry>,
}

#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct StorageEntry {
    key: B256,
    value: B256,
}

impl Compact for AlloyGenesisAccount {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let account = GenesisAccount {
            nonce: self.nonce,
            balance: self.balance,
            code: self.code,
            storage: self.storage.map(|s| StorageEntries {
                entries: s.into_iter().map(|(key, value)| StorageEntry { key, value }).collect(),
            }),
            private_key: self.private_key,
        };
        account.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (account, _) = GenesisAccount::from_compact(buf, len);
        let alloy_account = AlloyGenesisAccount {
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
