//! Streams BAL-derived final per-account state to the sparse-trie task.
//!
//! This is the provider-backed BAL state-root path used by parallel BAL execution. It mirrors
//! the serial BAL prewarm stream: storage updates come directly from BAL deltas, while
//! unchanged account fields are read from the parent-state provider through the shared
//! execution cache.

use crate::tree::{
    payload_processor::multiproof::StateRootMessage, CachedStateMetrics, CachedStateProvider,
    SavedCache, StateProviderBuilder,
};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::{bal::DecodedBal, AccountChanges};
use alloy_primitives::{keccak256, U256};
use crossbeam_channel::Sender as CrossbeamSender;
use rayon::prelude::*;
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, NodePrimitives};
use reth_provider::{
    AccountReader, BlockReader, StateProviderBox, StateProviderFactory, StateReader,
};
use reth_tasks::pool::WorkerPool;
use std::sync::Arc;
use tokio::sync::oneshot;

type ThreadCachedProvider = CachedStateProvider<StateProviderBox, true>;

/// Spawns the streaming task on `pool`.
///
/// Iterates the BAL in parallel, sends one `StateRootMessage::HashedStateUpdate` per changed
/// account, then sends `FinishedStateUpdates`. Provider/cache errors are returned through the
/// oneshot receiver so validation can fail before using an incomplete sparse-trie result.
pub fn spawn_stream_bal_to_sparse_trie<N, P>(
    pool: &WorkerPool,
    provider_builder: StateProviderBuilder<N, P>,
    saved_cache: SavedCache,
    cache_metrics: CachedStateMetrics,
    decoded_bal: Arc<DecodedBal>,
    to_sparse_trie_task: CrossbeamSender<StateRootMessage>,
) -> oneshot::Receiver<ProviderResult<()>>
where
    N: NodePrimitives + 'static,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    let (tx, rx) = oneshot::channel();
    pool.spawn(move || {
        let result = decoded_bal
            .as_bal()
            .par_iter()
            .map_init(
                || init_thread_provider(&provider_builder, &saved_cache, &cache_metrics),
                |provider_result, account_changes| {
                    let provider = provider_result.as_ref().map_err(Clone::clone)?;
                    stream_bal_account(provider, account_changes, &to_sparse_trie_task)
                },
            )
            .collect::<ProviderResult<()>>();

        let _ = to_sparse_trie_task.send(StateRootMessage::FinishedStateUpdates);
        let _ = tx.send(result);
    });
    rx
}

fn init_thread_provider<N, P>(
    builder: &StateProviderBuilder<N, P>,
    saved_cache: &SavedCache,
    cache_metrics: &CachedStateMetrics,
) -> ProviderResult<ThreadCachedProvider>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
{
    let built = builder.build()?;
    Ok(CachedStateProvider::new_prewarm(built, saved_cache.cache().clone(), cache_metrics.clone()))
}

/// Streams a single BAL account's post-block state. Sends a storage update if
/// `storage_changes` is non-empty. Sends an account update if any of balance, nonce, code, or
/// storage changed. Sends nothing for pure-read accounts.
fn stream_bal_account(
    account_reader: &impl AccountReader,
    account_changes: &AccountChanges,
    to_sparse_trie_task: &CrossbeamSender<StateRootMessage>,
) -> ProviderResult<()> {
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
        return Ok(());
    }

    let existing_account = account_reader.basic_account(&address)?;

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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange};
    use alloy_primitives::{Address, Bytes};
    use crossbeam_channel::unbounded;
    use reth_errors::ProviderResult;
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestAccountReader {
        accounts: HashMap<Address, Option<Account>>,
    }

    impl AccountReader for TestAccountReader {
        fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
            Ok(self.accounts.get(address).copied().flatten())
        }
    }

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    fn drain(rx: &crossbeam_channel::Receiver<StateRootMessage>) -> Vec<StateRootMessage> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    #[test]
    fn pure_read_account_sends_nothing() {
        let (tx, rx) = unbounded();
        let account_changes = AccountChanges::new(addr(1)).with_storage_read(U256::from(7));

        stream_bal_account(&TestAccountReader::default(), &account_changes, &tx).unwrap();
        assert!(drain(&rx).is_empty());
    }

    #[test]
    fn balance_only_change_sends_account_update() {
        let (tx, rx) = unbounded();
        let account_changes = AccountChanges::new(addr(1))
            .with_balance_change(BalanceChange::new(1, U256::from(100)));

        stream_bal_account(&TestAccountReader::default(), &account_changes, &tx).unwrap();

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
        let (tx, rx) = unbounded();
        let mut reader = TestAccountReader::default();
        reader.accounts.insert(
            addr(1),
            Some(Account { balance: U256::from(50), nonce: 3, bytecode_hash: None }),
        );
        let account_changes = AccountChanges::new(addr(1)).with_storage_change(SlotChanges::new(
            U256::from(7),
            vec![StorageChange::new(1, U256::from(99))],
        ));

        stream_bal_account(&reader, &account_changes, &tx).unwrap();

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

        stream_bal_account(&TestAccountReader::default(), &account_changes, &tx).unwrap();

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

        stream_bal_account(&TestAccountReader::default(), &account_changes, &tx).unwrap();

        let msgs = drain(&rx);
        let StateRootMessage::HashedStateUpdate(state) = &msgs[0] else {
            panic!("expected HashedStateUpdate");
        };
        let account = state.accounts.values().next().unwrap().as_ref().unwrap();
        assert_eq!(account.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn missing_provider_account_uses_zero_defaults() {
        let (tx, rx) = unbounded();
        let account_changes =
            AccountChanges::new(addr(1)).with_nonce_change(NonceChange::new(1, 1));

        stream_bal_account(&TestAccountReader::default(), &account_changes, &tx).unwrap();

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
