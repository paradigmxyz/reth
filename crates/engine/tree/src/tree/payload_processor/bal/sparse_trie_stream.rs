//! Streams BAL-derived final per-account state to the sparse-trie task.
//!
//! Reads account info from the pre-block [`BlockPreState`] snapshot. Does not query the
//! database. Composes the post-block account from BAL deltas (balance, nonce, code) and the
//! snapshot's pre-block fields. Sends one `HashedStateUpdate` per account, then closes the
//! stream with `FinishedStateUpdates`.

use super::pre_state::BlockPreState;
use crate::tree::payload_processor::multiproof::StateRootMessage;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::{AccountChanges, BlockAccessList};
use alloy_primitives::{keccak256, U256};
use crossbeam_channel::Sender as CrossbeamSender;
use rayon::prelude::*;
use reth_primitives_traits::Account;
use reth_tasks::pool::WorkerPool;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Spawns the streaming task on `pool`. Iterates the BAL in parallel, sends one
/// `StateRootMessage::HashedStateUpdate` per changed account, then sends
/// `FinishedStateUpdates`. Returns a receiver that fires once the task is done.
pub fn spawn_stream_bal_to_sparse_trie(
    pool: &WorkerPool,
    snapshot: Arc<BlockPreState>,
    bal: Arc<BlockAccessList>,
    to_sparse_trie_task: CrossbeamSender<StateRootMessage>,
) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    pool.spawn(move || {
        bal.par_iter().for_each(|account_changes| {
            stream_bal_account(&snapshot, account_changes, &to_sparse_trie_task);
        });
        let _ = to_sparse_trie_task.send(StateRootMessage::FinishedStateUpdates);
        let _ = tx.send(());
    });
    rx
}

/// Streams a single BAL account's post-block state. Sends a storage update if
/// `storage_changes` is non-empty. Sends an account update if any of balance, nonce, code, or
/// storage changed. Sends nothing for pure-read accounts (only `storage_reads` declared).
fn stream_bal_account(
    snapshot: &BlockPreState,
    account_changes: &AccountChanges,
    to_sparse_trie_task: &CrossbeamSender<StateRootMessage>,
) {
    let address = account_changes.address;
    let mut hashed_address = None;

    if !account_changes.storage_changes.is_empty() {
        let hashed = *hashed_address.get_or_insert_with(|| keccak256(address));
        let mut storage_map = reth_trie::HashedStorage::new(false);

        for slot_changes in &account_changes.storage_changes {
            let hashed_slot = keccak256(slot_changes.slot.to_be_bytes::<32>());
            if let Some(last_change) = slot_changes.changes.last() {
                storage_map.storage.insert(hashed_slot, last_change.new_value);
            }
        }

        let mut hashed_state = reth_trie::HashedPostState::default();
        hashed_state.storages.insert(hashed, storage_map);
        let _ = to_sparse_trie_task.send(StateRootMessage::HashedStateUpdate(hashed_state));
    }

    let balance = account_changes.balance_changes.last().map(|c| c.post_balance);
    let nonce = account_changes.nonce_changes.last().map(|c| c.new_nonce);
    let code_hash = account_changes.code_changes.last().map(|code_change| {
        if code_change.new_code.is_empty() {
            KECCAK_EMPTY
        } else {
            keccak256(&code_change.new_code)
        }
    });

    if balance.is_none() &&
        nonce.is_none() &&
        code_hash.is_none() &&
        account_changes.storage_changes.is_empty()
    {
        return;
    }

    // Pre-block account from snapshot. Missing entry means the account did not exist pre-block
    // (a CREATE target), and balance/nonce default to zero, code defaults to KECCAK_EMPTY.
    let existing_account = snapshot.accounts.get(&address).copied().flatten();

    let account = Account {
        balance: balance
            .unwrap_or_else(|| existing_account.as_ref().map(|a| a.balance).unwrap_or(U256::ZERO)),
        nonce: nonce.unwrap_or_else(|| existing_account.as_ref().map(|a| a.nonce).unwrap_or(0)),
        bytecode_hash: code_hash.or_else(|| {
            existing_account.as_ref().and_then(|a| a.bytecode_hash).or(Some(KECCAK_EMPTY))
        }),
    };

    let hashed_address = hashed_address.unwrap_or_else(|| keccak256(address));
    let mut hashed_state = reth_trie::HashedPostState::default();
    hashed_state.accounts.insert(hashed_address, Some(account));

    let _ = to_sparse_trie_task.send(StateRootMessage::HashedStateUpdate(hashed_state));
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange};
    use alloy_primitives::{Address, Bytes};
    use crossbeam_channel::unbounded;

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn empty_snapshot() -> BlockPreState {
        BlockPreState::default()
    }

    /// Drains all messages from the channel into a vector for inspection.
    fn drain(rx: &crossbeam_channel::Receiver<StateRootMessage>) -> Vec<StateRootMessage> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    #[test]
    fn pure_read_account_sends_nothing() {
        // Only storage_reads, no changes of any kind. Function must skip.
        let (tx, rx) = unbounded();
        let account_changes = AccountChanges::new(addr(1)).with_storage_read(U256::from(7));

        stream_bal_account(&empty_snapshot(), &account_changes, &tx);
        assert!(drain(&rx).is_empty());
    }

    #[test]
    fn balance_only_change_sends_account_update() {
        // BAL has balance change, no storage. One account update, no storage update.
        let (tx, rx) = unbounded();
        let account_changes = AccountChanges::new(addr(1))
            .with_balance_change(BalanceChange::new(1, U256::from(100)));

        stream_bal_account(&empty_snapshot(), &account_changes, &tx);

        let msgs = drain(&rx);
        assert_eq!(msgs.len(), 1, "expected exactly one update");
        let StateRootMessage::HashedStateUpdate(state) = &msgs[0] else {
            panic!("expected HashedStateUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(state.accounts.len(), 1);
        assert!(state.storages.is_empty());

        let account = state.accounts.values().next().unwrap().as_ref().unwrap();
        assert_eq!(account.balance, U256::from(100));
        assert_eq!(account.nonce, 0);
        assert_eq!(account.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn storage_change_sends_storage_and_account_updates() {
        // BAL has storage change. Two updates expected: storage and account (with current
        // balance/nonce/code from snapshot).
        let (tx, rx) = unbounded();
        let mut snapshot = empty_snapshot();
        snapshot.accounts.insert(
            addr(1),
            Some(Account { balance: U256::from(50), nonce: 3, bytecode_hash: None }),
        );
        let account_changes = AccountChanges::new(addr(1)).with_storage_change(SlotChanges::new(
            U256::from(7),
            vec![StorageChange::new(1, U256::from(99))],
        ));

        stream_bal_account(&snapshot, &account_changes, &tx);

        let msgs = drain(&rx);
        assert_eq!(msgs.len(), 2, "expected one storage and one account update");

        let mut got_storage = false;
        let mut got_account = false;
        for msg in &msgs {
            let StateRootMessage::HashedStateUpdate(state) = msg else {
                panic!("expected HashedStateUpdate");
            };
            if !state.storages.is_empty() {
                got_storage = true;
                let storage = state.storages.values().next().unwrap();
                assert_eq!(storage.storage.len(), 1);
                assert_eq!(*storage.storage.values().next().unwrap(), U256::from(99));
            }
            if !state.accounts.is_empty() {
                got_account = true;
                let account = state.accounts.values().next().unwrap().as_ref().unwrap();
                // Account fields fall back to snapshot values when unchanged.
                assert_eq!(account.balance, U256::from(50));
                assert_eq!(account.nonce, 3);
            }
        }
        assert!(got_storage, "missing storage update");
        assert!(got_account, "missing account update");
    }

    #[test]
    fn code_change_uses_keccak_of_new_code() {
        let (tx, rx) = unbounded();
        let new_code = Bytes::from_static(&[0x60, 0x00]);
        let expected_hash = keccak256(&new_code);
        let account_changes =
            AccountChanges::new(addr(1)).with_code_change(CodeChange::new(1, new_code));

        stream_bal_account(&empty_snapshot(), &account_changes, &tx);

        let msgs = drain(&rx);
        assert_eq!(msgs.len(), 1);
        let StateRootMessage::HashedStateUpdate(state) = &msgs[0] else {
            panic!("expected HashedStateUpdate");
        };
        let account = state.accounts.values().next().unwrap().as_ref().unwrap();
        assert_eq!(account.bytecode_hash, Some(expected_hash));
    }

    #[test]
    fn empty_new_code_uses_keccak_empty() {
        let (tx, rx) = unbounded();
        let account_changes =
            AccountChanges::new(addr(1)).with_code_change(CodeChange::new(1, Bytes::new()));

        stream_bal_account(&empty_snapshot(), &account_changes, &tx);

        let msgs = drain(&rx);
        let StateRootMessage::HashedStateUpdate(state) = &msgs[0] else {
            panic!("expected HashedStateUpdate");
        };
        let account = state.accounts.values().next().unwrap().as_ref().unwrap();
        assert_eq!(account.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn missing_snapshot_account_uses_zero_defaults() {
        // Account not in snapshot (CREATE target). balance/nonce default to 0, code to
        // KECCAK_EMPTY.
        let (tx, rx) = unbounded();
        let account_changes =
            AccountChanges::new(addr(1)).with_nonce_change(NonceChange::new(1, 1));

        stream_bal_account(&empty_snapshot(), &account_changes, &tx);

        let msgs = drain(&rx);
        let StateRootMessage::HashedStateUpdate(state) = &msgs[0] else {
            panic!("expected HashedStateUpdate");
        };
        let account = state.accounts.values().next().unwrap().as_ref().unwrap();
        assert_eq!(account.balance, U256::ZERO);
        assert_eq!(account.nonce, 1);
        assert_eq!(account.bytecode_hash, Some(KECCAK_EMPTY));
    }
}
