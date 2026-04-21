//! Revm [`Database`] adapter over an [`Arc<BlockPreState>`].
//!
//! `SnapshotDatabase` is the pre-block fallback layer for revm's `BalDatabase`. The worker's
//! read chain is:
//!
//! ```text
//! EVM → State → BalDatabase(received_bal, fallback = SnapshotDatabase)
//! ```
//!
//! `BalDatabase` resolves any slot/account the block legitimately touches — either from an
//! earlier in-block write (`block_access_index < tx_i`) or by delegating to us for the
//! pre-block value. Reads for keys not declared in the BAL short-circuit inside `BalDatabase`
//! with `BalError::AccountNotFound` and never reach us.
//!
//! This means a miss here is always a snapshot-fill bug: the caller built a snapshot that
//! didn't cover every key the BAL declared. We surface it as a [`SnapshotDbError`] rather than
//! silently returning defaults; the worker will propagate it upward as an engine internal
//! error (distinct from `RejectReason::UndeclaredAccess`, which comes from `BalDatabase`).

use super::pre_state::BlockPreState;
use alloy_primitives::{Address, B256, U256};
use revm::{bytecode::Bytecode, state::AccountInfo, Database, DatabaseRef};
use revm_database_interface::DBErrorMarker;
use std::sync::Arc;

/// Revm `Database` / `DatabaseRef` impl backed by an immutable `BlockPreState` snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotDatabase {
    snapshot: Arc<BlockPreState>,
}

impl SnapshotDatabase {
    /// Wraps a pre-built snapshot. Cheap; workers each hold their own `SnapshotDatabase` around
    /// a clone of the `Arc`.
    pub const fn new(snapshot: Arc<BlockPreState>) -> Self {
        Self { snapshot }
    }
}

impl Database for SnapshotDatabase {
    type Error = SnapshotDbError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash_ref(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_hash_ref(number)
    }
}

impl DatabaseRef for SnapshotDatabase {
    type Error = SnapshotDbError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        match self.snapshot.accounts.get(&address) {
            // Account existed pre-block. `Into::into` sets code=None so revm pulls bytecode
            // lazily via code_by_hash_ref when it needs it.
            Some(Some(account)) => Ok(Some((*account).into())),
            // Negative entry: the BAL declared this address but it didn't exist pre-block (a
            // CREATE target, or a touched-but-absent account). revm represents this as None.
            Some(None) => Ok(None),
            None => Err(SnapshotDbError::AccountMissing(address)),
        }
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        match self.snapshot.code.get(&code_hash) {
            // reth's Bytecode wraps revm's Bytecode in a newtype; .0 is the inner revm type.
            Some(Some(bytecode)) => Ok(bytecode.0.clone()),
            Some(None) => Ok(Bytecode::default()),
            None => Err(SnapshotDbError::CodeMissing(code_hash)),
        }
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let slot = B256::new(index.to_be_bytes());
        self.snapshot
            .storage
            .get(&(address, slot))
            .copied()
            .ok_or(SnapshotDbError::StorageMissing { address, slot })
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.snapshot
            .block_hashes
            .get(&number)
            .copied()
            .ok_or(SnapshotDbError::BlockHashMissing(number))
    }
}

/// Errors produced by [`SnapshotDatabase`].
///
/// Each variant means the snapshot was asked for a key it didn't contain. Since `BalDatabase`
/// catches undeclared-access attempts upstream, every variant here indicates a snapshot-fill
/// bug: the required-reads derivation missed a key the block legitimately needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotDbError {
    /// Basic-account lookup for an address the snapshot didn't materialize.
    AccountMissing(Address),
    /// Storage lookup for a `(address, slot)` pair the snapshot didn't materialize.
    StorageMissing {
        /// Address whose storage was queried.
        address: Address,
        /// 32-byte storage key the EVM asked for.
        slot: B256,
    },
    /// Code lookup for a hash absent from the snapshot's code map.
    CodeMissing(B256),
    /// `BLOCKHASH` opcode read of a block number not covered by the snapshot's window.
    BlockHashMissing(u64),
}

impl core::fmt::Display for SnapshotDbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AccountMissing(addr) => {
                write!(f, "account {addr} missing from BAL snapshot")
            }
            Self::StorageMissing { address, slot } => {
                write!(f, "storage ({address}, {slot}) missing from BAL snapshot")
            }
            Self::CodeMissing(hash) => write!(f, "code {hash} missing from BAL snapshot"),
            Self::BlockHashMissing(number) => {
                write!(f, "block_hash {number} missing from BAL snapshot")
            }
        }
    }
}

impl core::error::Error for SnapshotDbError {}

impl DBErrorMarker for SnapshotDbError {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_primitives::{keccak256, Bytes, StorageKey};
    use reth_primitives_traits::{Account, Bytecode as RethBytecode};

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn slot_key(byte: u8) -> StorageKey {
        let mut b = [0u8; 32];
        b[31] = byte;
        B256::from(b)
    }

    fn snapshot_with<F: FnOnce(&mut BlockPreState)>(f: F) -> Arc<BlockPreState> {
        let mut pre = BlockPreState::default();
        f(&mut pre);
        Arc::new(pre)
    }

    #[test]
    fn basic_contract_account_returns_populated_info() {
        let bytecode_bytes = Bytes::from_static(&[0x60, 0x00]);
        let code_hash = keccak256(&bytecode_bytes);
        let pre = snapshot_with(|p| {
            p.accounts.insert(
                addr(1),
                Some(Account {
                    balance: U256::from(123),
                    nonce: 7,
                    bytecode_hash: Some(code_hash),
                }),
            );
        });
        let mut db = SnapshotDatabase::new(pre);

        let info = db.basic(addr(1)).unwrap().expect("account present");
        assert_eq!(info.balance, U256::from(123));
        assert_eq!(info.nonce, 7);
        assert_eq!(info.code_hash, code_hash);
        // Bytecode is not eagerly loaded; revm fetches it lazily via code_by_hash.
        assert!(info.code.is_none());
    }

    #[test]
    fn basic_eoa_has_empty_code_hash() {
        let pre = snapshot_with(|p| {
            p.accounts.insert(
                addr(1),
                Some(Account { balance: U256::ZERO, nonce: 0, bytecode_hash: None }),
            );
        });
        let mut db = SnapshotDatabase::new(pre);

        let info = db.basic(addr(1)).unwrap().expect("account present");
        assert_eq!(info.code_hash, KECCAK_EMPTY);
    }

    #[test]
    fn basic_negative_entry_is_ok_none() {
        // BAL declared the address, but it didn't exist pre-block (e.g., CREATE target).
        let pre = snapshot_with(|p| {
            p.accounts.insert(addr(1), None);
        });
        let mut db = SnapshotDatabase::new(pre);

        assert_eq!(db.basic(addr(1)).unwrap(), None);
    }

    #[test]
    fn basic_missing_is_error() {
        let mut db = SnapshotDatabase::new(snapshot_with(|_| {}));
        assert_eq!(db.basic(addr(1)).unwrap_err(), SnapshotDbError::AccountMissing(addr(1)));
    }

    #[test]
    fn storage_hit() {
        let pre = snapshot_with(|p| {
            p.storage.insert((addr(1), slot_key(10)), U256::from(42));
        });
        let mut db = SnapshotDatabase::new(pre);

        assert_eq!(db.storage(addr(1), U256::from(10)).unwrap(), U256::from(42));
    }

    #[test]
    fn storage_zero_entry_returns_zero_not_error() {
        // A slot whose pre-block value is literally zero is a legitimate read; must not
        // degrade to "missing".
        let pre = snapshot_with(|p| {
            p.storage.insert((addr(1), slot_key(10)), U256::ZERO);
        });
        let mut db = SnapshotDatabase::new(pre);

        assert_eq!(db.storage(addr(1), U256::from(10)).unwrap(), U256::ZERO);
    }

    #[test]
    fn storage_missing_populates_both_fields() {
        let mut db = SnapshotDatabase::new(snapshot_with(|_| {}));
        let err = db.storage(addr(1), U256::from(10)).unwrap_err();
        assert_eq!(err, SnapshotDbError::StorageMissing { address: addr(1), slot: slot_key(10) });
    }

    #[test]
    fn code_by_hash_hit() {
        let bytecode_bytes = Bytes::from_static(&[0xfe, 0x01]);
        let code_hash = keccak256(&bytecode_bytes);
        let pre = snapshot_with(|p| {
            p.code.insert(code_hash, Some(RethBytecode::new_raw(bytecode_bytes.clone())));
        });
        let mut db = SnapshotDatabase::new(pre);

        let got = db.code_by_hash(code_hash).unwrap();
        assert_eq!(got.original_byte_slice(), &bytecode_bytes[..]);
    }

    #[test]
    fn code_by_hash_empty_entry_is_default() {
        // Account had bytecode_hash = KECCAK_EMPTY → snapshot stored Some(None) (empty code).
        let pre = snapshot_with(|p| {
            p.code.insert(KECCAK_EMPTY, None);
        });
        let mut db = SnapshotDatabase::new(pre);

        assert_eq!(db.code_by_hash(KECCAK_EMPTY).unwrap(), Bytecode::default());
    }

    #[test]
    fn code_by_hash_missing_is_error() {
        let hash = B256::repeat_byte(0xcc);
        let mut db = SnapshotDatabase::new(snapshot_with(|_| {}));
        assert_eq!(db.code_by_hash(hash).unwrap_err(), SnapshotDbError::CodeMissing(hash));
    }

    #[test]
    fn block_hash_hit_and_miss() {
        let pre = snapshot_with(|p| {
            p.block_hashes.insert(5, B256::repeat_byte(0xaa));
        });
        let mut db = SnapshotDatabase::new(pre);

        assert_eq!(db.block_hash(5).unwrap(), B256::repeat_byte(0xaa));
        assert_eq!(db.block_hash(6).unwrap_err(), SnapshotDbError::BlockHashMissing(6));
    }

    #[test]
    fn database_ref_impls_agree_with_database() {
        // DatabaseRef methods share the same logic (Database forwards to them), but sanity
        // check that &self access works without needing &mut.
        let pre = snapshot_with(|p| {
            p.accounts.insert(
                addr(1),
                Some(Account { balance: U256::from(1), nonce: 0, bytecode_hash: None }),
            );
        });
        let db = SnapshotDatabase::new(pre);
        assert!(db.basic_ref(addr(1)).unwrap().is_some());
    }
}
