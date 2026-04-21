//! In-memory pre-block state snapshot consumed by BAL workers.
//!
//! Workers run their EVMs against an immutable `Arc<BlockPreState>` rather than a live
//! `StateProvider`. The snapshot is built once per block from the BAL's declared access set;
//! every read a worker can legitimately perform is satisfied from it.

use alloc::collections::BTreeSet;
use alloy_eip7928::AccountChanges;
use alloy_primitives::{map::B256HashMap, Address, StorageKey, StorageValue, B256, U256};
use reth_primitives_traits::{Account, Bytecode};
use std::collections::HashMap;

/// Immutable pre-block state materialized in memory for every key the block reads.
///
/// Populated once per block by the snapshot-build phase (consulting the cross-block execution
/// cache with fallback to the state provider). Shared across workers as `Arc<BlockPreState>`.
///
/// Storage keys are `(Address, StorageKey)`-indexed. Code is keyed by `code_hash`, populated
/// after accounts are resolved (since `code_hash` comes from the basic account).
#[derive(Debug, Default, Clone)]
pub struct BlockPreState {
    /// Pre-block account state. `None` means the account did not exist pre-block.
    pub accounts: HashMap<Address, Option<Account>>,

    /// Pre-block storage values. Missing entries are treated as zero at lookup time only if the
    /// miss is legitimate; a miss while executing always indicates an undeclared access.
    pub storage: HashMap<(Address, StorageKey), StorageValue>,

    /// Pre-block code, keyed by `code_hash`. `None` denotes "no code" (EOA / empty account).
    pub code: B256HashMap<Option<Bytecode>>,

    /// Recent block hashes available via the `BLOCKHASH` opcode. Derived from headers, not BAL.
    pub block_hashes: HashMap<u64, B256>,
}

impl BlockPreState {
    /// Returns pre-block account data for `address`, or `None` if the address was not declared
    /// in the BAL used to seed this snapshot.
    #[inline]
    pub fn account(&self, address: &Address) -> Option<&Option<Account>> {
        self.accounts.get(address)
    }

    /// Returns pre-block storage value for `(address, slot)`, or `None` if not declared.
    #[inline]
    pub fn storage_value(&self, address: &Address, slot: &StorageKey) -> Option<StorageValue> {
        self.storage.get(&(*address, *slot)).copied()
    }

    /// Returns pre-block code for `code_hash`, or `None` if not present.
    #[inline]
    pub fn code(&self, code_hash: &B256) -> Option<&Option<Bytecode>> {
        self.code.get(code_hash)
    }
}

/// The exhaustive set of pre-block keys the snapshot must materialize, derived from a BAL.
///
/// Fetching is two-phase because code lookups are keyed by `code_hash`, which comes from the
/// basic account. Callers first fill `accounts` and `storage` in parallel, then use the resolved
/// accounts to populate the set of `code_hashes` to fetch.
///
/// `block_hashes` are independent of BAL and populated directly from recent headers.
#[derive(Debug, Default, Clone)]
pub struct RequiredReads {
    /// Addresses needing a basic-account fetch. Every address in the BAL appears here.
    pub addresses: BTreeSet<Address>,

    /// Storage slots needing a pre-block value fetch: the union of `storage_reads` and
    /// `storage_changes` slots, per EIP-7928 (disjoint per account), across all BAL accounts.
    pub storage: BTreeSet<(Address, StorageKey)>,
}

impl RequiredReads {
    /// Derives the required-reads set from a BAL.
    ///
    /// Addresses and storage slots are collected into `BTreeSet`s for deterministic iteration
    /// order (useful for reproducible tests and ordered parallel fill).
    pub fn from_bal<'a, I>(bal: I) -> Self
    where
        I: IntoIterator<Item = &'a AccountChanges>,
    {
        let mut addresses = BTreeSet::new();
        let mut storage = BTreeSet::new();

        for account_changes in bal {
            let address = account_changes.address;
            addresses.insert(address);

            for slot_changes in &account_changes.storage_changes {
                storage.insert((address, storage_key_from_u256(slot_changes.slot)));
            }
            for slot in &account_changes.storage_reads {
                storage.insert((address, storage_key_from_u256(*slot)));
            }
        }

        Self { addresses, storage }
    }
}

/// Converts a BAL `U256` slot identifier to the reth `StorageKey` (`B256`) representation.
#[inline]
fn storage_key_from_u256(slot: U256) -> StorageKey {
    B256::from(slot.to_be_bytes::<32>())
}

// Import vec/alloc for no_std compatibility in tests below.
extern crate alloc;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange};
    use alloy_primitives::{address, b256, Bytes, U256};

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn slot(byte: u8) -> U256 {
        U256::from(byte)
    }

    #[test]
    fn required_reads_collects_unique_addresses() {
        let bal = vec![
            AccountChanges::new(addr(1)),
            AccountChanges::new(addr(2)),
            AccountChanges::new(addr(1)),
        ];

        let reads = RequiredReads::from_bal(&bal);

        assert_eq!(reads.addresses.len(), 2);
        assert!(reads.addresses.contains(&addr(1)));
        assert!(reads.addresses.contains(&addr(2)));
    }

    #[test]
    fn required_reads_unions_storage_reads_and_writes() {
        let account = AccountChanges::new(addr(1))
            .with_storage_read(slot(10))
            .with_storage_read(slot(11))
            .with_storage_change(SlotChanges::new(
                slot(20),
                vec![StorageChange::new(1, U256::from(42))],
            ))
            .with_storage_change(SlotChanges::new(
                slot(21),
                vec![StorageChange::new(2, U256::from(43))],
            ));

        let reads = RequiredReads::from_bal(&[account]);

        assert_eq!(reads.storage.len(), 4);
        assert!(reads.storage.contains(&(addr(1), storage_key_from_u256(slot(10)))));
        assert!(reads.storage.contains(&(addr(1), storage_key_from_u256(slot(20)))));
    }

    #[test]
    fn required_reads_ignores_non_storage_changes() {
        // Balance/nonce/code changes don't contribute to the required-reads storage set.
        // They contribute only by virtue of their address appearing in `addresses`.
        let account = AccountChanges::new(addr(1))
            .with_balance_change(BalanceChange::new(1, U256::from(100)))
            .with_nonce_change(NonceChange::new(1, 7))
            .with_code_change(CodeChange::new(1, Bytes::from_static(&[0x00])));

        let reads = RequiredReads::from_bal(&[account]);

        assert_eq!(reads.addresses.len(), 1);
        assert!(reads.storage.is_empty());
    }

    #[test]
    fn required_reads_empty_bal() {
        let reads = RequiredReads::from_bal(std::iter::empty::<&AccountChanges>());

        assert!(reads.addresses.is_empty());
        assert!(reads.storage.is_empty());
    }

    #[test]
    fn required_reads_multi_account_storage() {
        let bal = vec![
            AccountChanges::new(addr(1)).with_storage_read(slot(10)),
            AccountChanges::new(addr(2)).with_storage_read(slot(10)),
        ];

        let reads = RequiredReads::from_bal(&bal);

        // Same slot for different addresses: both must appear.
        assert_eq!(reads.storage.len(), 2);
        assert!(reads.storage.contains(&(addr(1), storage_key_from_u256(slot(10)))));
        assert!(reads.storage.contains(&(addr(2), storage_key_from_u256(slot(10)))));
    }

    #[test]
    fn storage_key_conversion_is_big_endian() {
        let s = slot(1);
        let k = storage_key_from_u256(s);
        assert_eq!(k, b256!("0x0000000000000000000000000000000000000000000000000000000000000001"));
    }

    #[test]
    fn block_pre_state_lookups() {
        let mut pre = BlockPreState::default();
        pre.accounts.insert(
            addr(1),
            Some(Account { balance: U256::from(100), nonce: 1, bytecode_hash: None }),
        );
        pre.accounts.insert(addr(2), None);
        pre.storage.insert((addr(1), storage_key_from_u256(slot(10))), U256::from(42));

        assert!(pre.account(&addr(1)).and_then(|a| a.as_ref()).is_some());
        assert!(pre.account(&addr(2)).is_some_and(|a| a.is_none()));
        assert!(pre.account(&address!("0x0000000000000000000000000000000000000099")).is_none());
        assert_eq!(
            pre.storage_value(&addr(1), &storage_key_from_u256(slot(10))),
            Some(U256::from(42))
        );
        assert_eq!(pre.storage_value(&addr(1), &storage_key_from_u256(slot(11))), None);
    }
}
