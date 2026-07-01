//! State root task interface types shared between the engine tree and the payload builder.

use crate::root::ParallelStateRootError;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::B256;
use derive_more::derive::Deref;
use reth_trie::{updates::TrieUpdates, HashedPostState, MultiProofTargetsV2};
use std::sync::Arc;

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub enum StateRootMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargetsV2),
    /// Pre-hashed state update that can be applied directly without proofs.
    HashedStateUpdate(HashedPostState),
    /// Block Access List (EIP-7928; BAL) containing complete state changes for the block.
    ///
    /// When received, the task generates a single state update from the BAL and processes it.
    /// No further messages are expected after receiving this variant.
    BlockAccessList(Arc<BlockAccessList>),
    /// Signals state update stream end.
    ///
    /// This is triggered by block execution, indicating that no additional state updates are
    /// expected.
    FinishedStateUpdates,
}

/// Outcome of the state root computation, including the state root itself with
/// the trie updates.
#[derive(Debug, Clone)]
pub struct StateRootComputeOutcome {
    /// The state root.
    pub state_root: B256,
    /// The trie updates.
    pub trie_updates: Arc<TrieUpdates>,
    /// Debug recorders taken from the sparse tries, keyed by `None` for account trie
    /// and `Some(address)` for storage tries.
    #[cfg(feature = "trie-debug")]
    pub debug_recorders: Vec<(Option<B256>, reth_trie_sparse::debug_recorder::TrieDebugRecorder)>,
}

/// Handle to a background sparse trie state root computation.
///
/// Used by both the engine (during `newPayload`) and the payload builder (during `FCU`-triggered
/// block building). Provides channels for streaming state updates into the pipeline and receiving
/// the final computed state root.
///
/// Created by `PayloadProcessor::spawn_state_root`.
#[derive(Debug)]
pub struct StateRootHandle {
    /// The state root that the cached sparse trie is anchored at (parent block's state root).
    cached_trie_state_root: B256,
    /// Channel for streaming state updates and proof targets into the sparse trie pipeline.
    updates_tx: crossbeam_channel::Sender<StateRootMessage>,
    /// Receiver for the final state root result.
    state_root_rx:
        Option<std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>>,
    /// Receiver for the hashed post state.
    hashed_state_rx: Option<std::sync::mpsc::Receiver<HashedPostState>>,
}

impl StateRootHandle {
    /// Creates a new [`StateRootHandle`].
    pub const fn new(
        cached_trie_state_root: B256,
        updates_tx: crossbeam_channel::Sender<StateRootMessage>,
        state_root_rx: std::sync::mpsc::Receiver<
            Result<StateRootComputeOutcome, ParallelStateRootError>,
        >,
        hashed_state_rx: std::sync::mpsc::Receiver<HashedPostState>,
    ) -> Self {
        Self {
            cached_trie_state_root,
            updates_tx,
            state_root_rx: Some(state_root_rx),
            hashed_state_rx: Some(hashed_state_rx),
        }
    }

    /// Returns the state root that the cached sparse trie is anchored at.
    pub const fn cached_trie_state_root(&self) -> B256 {
        self.cached_trie_state_root
    }

    /// Returns a reference to the updates sender channel.
    pub const fn updates_tx(&self) -> &crossbeam_channel::Sender<StateRootMessage> {
        &self.updates_tx
    }

    /// Returns a sender that streams hashed state updates to the background state root task.
    ///
    /// The sender must be dropped after execution completes to signal the end of state updates.
    pub fn state_hook_sender(&self) -> StateHookSender {
        StateHookSender::new(self.updates_tx.clone())
    }

    /// Awaits the state root computation result.
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub fn state_root(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        self.state_root_rx
            .take()
            .expect("state_root already taken")
            .recv()
            .map_err(|_| ParallelStateRootError::Other("sparse trie task dropped".to_string()))?
    }

    /// Takes the state root receiver for use with custom waiting logic (e.g., timeouts).
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub const fn take_state_root_rx(
        &mut self,
    ) -> std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>> {
        self.state_root_rx.take().expect("state_root already taken")
    }

    /// Takes the hashed state receiver
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub const fn take_hashed_state_rx(&mut self) -> std::sync::mpsc::Receiver<HashedPostState> {
        self.hashed_state_rx.take().expect("hashed_state already taken")
    }
}

/// A wrapper for the sender that signals completion when dropped.
///
/// This type is intended to be used in combination with the evm executor statehook.
/// This should trigger once the block has been executed (after) the last state update has been
/// sent. This triggers the exit condition of the multi proof task.
#[derive(Deref, Debug)]
pub struct StateHookSender(crossbeam_channel::Sender<StateRootMessage>);

impl StateHookSender {
    /// Creates a new [`StateHookSender`] wrapping the given channel sender.
    pub const fn new(inner: crossbeam_channel::Sender<StateRootMessage>) -> Self {
        Self(inner)
    }

    /// Sends a hashed state update to the background state root task.
    pub fn send_hashed_state(&self, state: HashedPostState) {
        if !state.is_empty() {
            let _ = self.0.send(StateRootMessage::HashedStateUpdate(state));
        }
    }
}

impl Drop for StateHookSender {
    fn drop(&mut self) {
        // Send completion signal when the sender is dropped
        let _ = self.0.send(StateRootMessage::FinishedStateUpdates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_primitives_traits::Account;

    #[test]
    fn state_hook_sender_streams_hashed_updates_and_finish() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let sender = StateHookSender::new(tx);

        sender.send_hashed_state(HashedPostState::default());
        assert!(rx.is_empty());

        let hashed_address = B256::repeat_byte(0x01);
        let account = Account { balance: U256::from(1), nonce: 1, bytecode_hash: None };
        let hashed_state =
            HashedPostState::default().with_accounts([(hashed_address, Some(account))]);

        sender.send_hashed_state(hashed_state.clone());
        drop(sender);

        let StateRootMessage::HashedStateUpdate(update) = rx.recv().unwrap() else {
            panic!("expected hashed state update");
        };
        assert_eq!(update, hashed_state);

        assert!(matches!(rx.recv().unwrap(), StateRootMessage::FinishedStateUpdates));
        assert!(rx.try_recv().is_err());
    }
}
