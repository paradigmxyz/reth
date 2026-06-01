//! Commonware LtHash state root helpers.

use alloc::vec::Vec;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{B256, U256};
use commonware_cryptography::lthash::LtHash;
use reth_primitives_traits::Account;

const STORAGE_SLOT_DOMAIN: u8 = 0;
const ACCOUNT_DOMAIN: u8 = 1;

/// LtHash accumulator for one account's storage slots.
#[derive(Debug, Default)]
pub struct LatticeStorageRoot {
    hash: LtHash,
    buf: Vec<u8>,
}

impl LatticeStorageRoot {
    /// Adds a storage slot to the storage accumulator.
    pub fn add_slot(&mut self, hashed_slot: B256, value: U256) {
        self.buf.clear();
        self.buf.reserve(1 + 32 + 32);
        self.buf.push(STORAGE_SLOT_DOMAIN);
        self.buf.extend_from_slice(hashed_slot.as_slice());
        self.buf.extend_from_slice(&value.to_be_bytes::<32>());
        self.hash.add(&self.buf);
    }

    /// Returns the checksum root for the accumulated storage slots.
    pub fn root(&self) -> B256 {
        checksum(&self.hash)
    }
}

/// LtHash accumulator for the global account state.
#[derive(Debug, Default)]
pub struct LatticeStateRoot {
    hash: LtHash,
    buf: Vec<u8>,
}

impl LatticeStateRoot {
    /// Adds an account to the global state accumulator.
    pub fn add_account(&mut self, hashed_address: B256, account: Account, storage_root: B256) {
        self.buf.clear();
        self.buf.reserve(1 + 32 + 8 + 32 + 32 + 32);
        self.buf.push(ACCOUNT_DOMAIN);
        self.buf.extend_from_slice(hashed_address.as_slice());
        self.buf.extend_from_slice(&account.nonce.to_be_bytes());
        self.buf.extend_from_slice(&account.balance.to_be_bytes::<32>());
        self.buf.extend_from_slice(account.bytecode_hash.unwrap_or(KECCAK_EMPTY).as_slice());
        self.buf.extend_from_slice(storage_root.as_slice());
        self.hash.add(&self.buf);
    }

    /// Returns the checksum root for the accumulated accounts.
    pub fn root(&self) -> B256 {
        checksum(&self.hash)
    }
}

fn checksum(hash: &LtHash) -> B256 {
    B256::from(hash.checksum().0)
}
