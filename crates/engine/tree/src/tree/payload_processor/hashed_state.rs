//! Streaming hashed post-state collector.

use crate::tree::payload_processor::multiproof::{
    evm_state_to_hashed_post_state, StateRootMessage,
};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_chain_state::DeferredTrieData;
use reth_tasks::Runtime;
use reth_trie::{updates::TrieUpdates, HashedPostState};
use std::sync::Arc;
use tracing::{debug, debug_span};

/// Handle for the streamed hashed post-state collector.
#[derive(Debug)]
pub(crate) struct StreamingHashedStateHandle {
    updates_tx: CrossbeamSender<StateRootMessage>,
    deferred_trie_data: DeferredTrieData,
}

impl StreamingHashedStateHandle {
    pub(crate) fn updates_tx(&self) -> CrossbeamSender<StateRootMessage> {
        self.updates_tx.clone()
    }

    pub(crate) fn deferred_trie_data(self) -> DeferredTrieData {
        self.deferred_trie_data
    }
}

/// Spawns a task that collects execution updates into the hashed post-state needed by persistence.
pub(crate) fn spawn_streaming_hashed_state_task(executor: &Runtime) -> StreamingHashedStateHandle {
    let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
    let (deferred_trie_data, publisher) = DeferredTrieData::pending_uncomputed();

    executor.spawn_blocking_named("hashed-post-state-stream", move || {
        let _span = debug_span!(
            target: "engine::tree::payload_processor::hashed_state",
            "hashed_post_state_stream"
        )
        .entered();

        let hashed_state = collect_hashed_state(updates_rx);
        let computed =
            DeferredTrieData::sort(Arc::new(hashed_state), Arc::new(TrieUpdates::default()));
        let _ = publisher.publish(computed);
    });

    StreamingHashedStateHandle { updates_tx, deferred_trie_data }
}

fn collect_hashed_state(updates: CrossbeamReceiver<StateRootMessage>) -> HashedPostState {
    let mut hashed_state = HashedPostState::default();

    while let Ok(message) = updates.recv() {
        match message {
            StateRootMessage::StateUpdate(state) => {
                hashed_state.extend(evm_state_to_hashed_post_state(state));
            }
            StateRootMessage::HashedStateUpdate(update) => {
                hashed_state.extend(update);
            }
            StateRootMessage::FinishedStateUpdates => break,
            StateRootMessage::PrefetchProofs(_) => {}
            StateRootMessage::BlockAccessList(_) => {
                debug!(
                    target: "engine::tree::payload_processor::hashed_state",
                    "Ignoring raw BAL in streamed hashed post-state collector"
                );
            }
        }
    }

    hashed_state
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, U256};
    use reth_primitives_traits::Account;
    use reth_trie::HashedStorage;

    #[test]
    fn collects_hashed_updates_until_finished() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let hashed_address = B256::repeat_byte(0x11);
        let hashed_slot = B256::repeat_byte(0x22);

        let mut update = HashedPostState::default();
        update.accounts.insert(
            hashed_address,
            Some(Account {
                nonce: 1,
                balance: U256::from(2),
                bytecode_hash: Some(B256::repeat_byte(0x33)),
            }),
        );
        update.storages.insert(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot, U256::from(3))]),
        );

        tx.send(StateRootMessage::HashedStateUpdate(update)).unwrap();
        tx.send(StateRootMessage::FinishedStateUpdates).unwrap();

        let collected = collect_hashed_state(rx);

        assert!(collected.accounts.contains_key(&hashed_address));
        assert_eq!(
            collected.storages[&hashed_address].storage.get(&hashed_slot),
            Some(&U256::from(3))
        );
    }
}
