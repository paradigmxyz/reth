//! State root task interface types shared between the engine tree and the payload builder.

use crate::root::ParallelStateRootError;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::{keccak256, B256};
use reth_trie::{updates::TrieUpdates, HashedPostState, HashedStorage, MultiProofTargetsV2};
use revm_state::EvmState;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tracing::{debug, trace};

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub enum StateRootMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargetsV2),
    /// New state update from transaction execution with its source
    StateUpdate(EvmState),
    /// Pre-hashed state update from BAL conversion that can be applied directly without proofs.
    HashedStateUpdate(HashedPostState),
    /// Error produced before updates could be fully streamed.
    Error(ParallelStateRootError),
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
    /// Set when the payload-builder side drops this handle before consuming sparse-trie outputs.
    consumer_cancelled: Arc<AtomicBool>,
    /// Optional gate that delays execution state updates until prerequisite updates are streamed.
    update_gate: Option<StateRootUpdateGate>,
}

impl StateRootHandle {
    /// Creates a new [`StateRootHandle`].
    pub fn new(
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
            consumer_cancelled: Arc::new(AtomicBool::new(false)),
            update_gate: None,
        }
    }

    /// Returns the state root that the cached sparse trie is anchored at.
    pub const fn cached_trie_state_root(&self) -> B256 {
        self.cached_trie_state_root
    }

    /// Returns whether this handle is waiting before sparse-trie work can run.
    pub const fn is_deferred(&self) -> bool {
        false
    }

    /// Returns whether this handle is still waiting for parent validation.
    pub const fn is_deferred_parent_pending(&self) -> bool {
        false
    }

    /// Returns a flag that is set once this handle is dropped by its consumer.
    pub fn consumer_cancelled(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.consumer_cancelled)
    }

    /// Returns a reference to the updates sender channel.
    pub const fn updates_tx(&self) -> &crossbeam_channel::Sender<StateRootMessage> {
        &self.updates_tx
    }

    /// Attaches a gate that must open before state-hook updates are sent.
    pub fn with_update_gate(mut self, update_gate: StateRootUpdateGate) -> Self {
        self.update_gate = Some(update_gate);
        self
    }

    /// Returns a state hook that streams state updates to the background state root task.
    ///
    /// The hook must be dropped after execution completes to signal the end of state updates.
    pub fn state_hook(&self) -> impl alloy_evm::block::OnStateHook {
        let sender = StateHookSender::new(self.updates_tx.clone(), self.update_gate.clone());

        move |state: &EvmState| {
            let _ = sender.send(StateRootMessage::StateUpdate(state.clone()));
        }
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

impl Drop for StateRootHandle {
    fn drop(&mut self) {
        self.consumer_cancelled.store(true, Ordering::Release);
    }
}

/// Gate used to preserve update ordering when prerequisite state is streamed asynchronously.
///
/// While closed, state-hook updates are buffered in memory instead of blocking the caller. This is
/// important for speculative payload builders: execution must be able to finish and drop its state
/// provider even if parent BAL streaming is still waiting for validation resources.
#[derive(Clone, Debug, Default)]
pub struct StateRootUpdateGate {
    inner: Arc<StateRootUpdateGateInner>,
}

#[derive(Debug, Default)]
struct StateRootUpdateGateInner {
    open: AtomicBool,
    pending: Mutex<Vec<StateRootMessage>>,
}

impl StateRootUpdateGate {
    /// Creates a closed update gate.
    pub fn closed() -> Self {
        Self::default()
    }

    /// Opens the gate and flushes any buffered state-hook updates after prerequisite updates.
    pub fn open(&self, sender: &crossbeam_channel::Sender<StateRootMessage>) {
        if self.inner.open.load(Ordering::Acquire) {
            return;
        }

        let mut pending =
            self.inner.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.inner.open.swap(true, Ordering::AcqRel) {
            return;
        }

        let pending_count = pending.len();
        debug!(
            target: "trie::parallel::sparse",
            pending_count,
            "flushing buffered sparse-trie state updates"
        );

        for message in pending.drain(..) {
            if sender.send(message).is_err() {
                break;
            }
        }
    }

    /// Sends immediately when open, otherwise buffers without blocking.
    fn send(
        &self,
        sender: &crossbeam_channel::Sender<StateRootMessage>,
        message: StateRootMessage,
    ) -> Result<(), crossbeam_channel::SendError<StateRootMessage>> {
        if self.inner.open.load(Ordering::Acquire) {
            return sender.send(message);
        }

        let mut pending =
            self.inner.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.inner.open.load(Ordering::Acquire) {
            drop(pending);
            return sender.send(message);
        }

        pending.push(message);
        Ok(())
    }
}

/// A wrapper for the sender that signals completion when dropped.
///
/// This type is intended to be used in combination with the evm executor statehook.
/// This should trigger once the block has been executed (after) the last state update has been
/// sent. This triggers the exit condition of the multi proof task.
#[derive(Debug)]
pub struct StateHookSender {
    sender: crossbeam_channel::Sender<StateRootMessage>,
    update_gate: Option<StateRootUpdateGate>,
}

impl StateHookSender {
    /// Creates a new [`StateHookSender`] wrapping the given channel sender.
    pub const fn new(
        sender: crossbeam_channel::Sender<StateRootMessage>,
        update_gate: Option<StateRootUpdateGate>,
    ) -> Self {
        Self { sender, update_gate }
    }

    /// Sends a message after any configured prerequisite update gate has opened.
    pub fn send(
        &self,
        message: StateRootMessage,
    ) -> Result<(), crossbeam_channel::SendError<StateRootMessage>> {
        if let Some(gate) = &self.update_gate {
            return gate.send(&self.sender, message);
        }
        self.sender.send(message)
    }
}

impl Drop for StateHookSender {
    fn drop(&mut self) {
        if let Some(gate) = &self.update_gate {
            let _ = gate.send(&self.sender, StateRootMessage::FinishedStateUpdates);
            return;
        }
        // Send completion signal when the sender is dropped
        let _ = self.sender.send(StateRootMessage::FinishedStateUpdates);
    }
}

/// Converts [`EvmState`] to [`HashedPostState`] by keccak256-hashing addresses and storage slots.
pub fn evm_state_to_hashed_post_state(update: EvmState) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.len());

    for (address, account) in update {
        if account.is_touched() {
            let hashed_address = keccak256(address);
            trace!(target: "trie::parallel::sparse", ?address, ?hashed_address, "Adding account to state update");

            let destroyed = account.is_selfdestructed();
            if account.info != account.original_info() {
                let info = if destroyed { None } else { Some(account.info.into()) };
                hashed_state.accounts.insert(hashed_address, info);
            }

            let mut changed_storage_iter = account
                .storage
                .into_iter()
                .filter(|(_slot, value)| value.is_changed())
                .map(|(slot, value)| (keccak256(B256::from(slot)), value.present_value))
                .peekable();

            if destroyed {
                hashed_state.storages.insert(hashed_address, HashedStorage::new(true));
            } else if changed_storage_iter.peek().is_some() {
                hashed_state
                    .storages
                    .insert(hashed_address, HashedStorage::from_iter(false, changed_storage_iter));
            }
        }
    }

    hashed_state
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::block::OnStateHook;

    #[test]
    fn state_root_handle_is_active_when_created() {
        let (updates_tx, _updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();

        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx, state_root_rx, hashed_state_rx);
        assert!(!handle.is_deferred());
        assert!(!handle.is_deferred_parent_pending());
    }

    #[test]
    fn state_root_handle_drop_signals_consumer_cancelled() {
        let (updates_tx, _updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx, state_root_rx, hashed_state_rx);
        let consumer_cancelled = handle.consumer_cancelled();

        assert!(!consumer_cancelled.load(Ordering::Acquire));
        drop(handle);
        assert!(consumer_cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn state_hook_streams_updates_immediately() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx, state_root_rx, hashed_state_rx);

        let mut hook = handle.state_hook();
        hook.on_state(&EvmState::default());
        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::StateUpdate(_)));

        drop(hook);
        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::FinishedStateUpdates));
    }

    #[test]
    fn state_hook_buffers_updates_until_update_gate_opens() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let gate = StateRootUpdateGate::closed();
        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx.clone(), state_root_rx, hashed_state_rx)
            .with_update_gate(gate.clone());
        let (started_tx, started_rx) = std::sync::mpsc::channel();

        let worker = std::thread::spawn(move || {
            let mut hook = handle.state_hook();
            started_tx.send(()).unwrap();
            hook.on_state(&EvmState::default());
            drop(hook);
        });

        started_rx.recv().unwrap();
        worker.join().unwrap();
        assert!(updates_rx.recv_timeout(std::time::Duration::from_millis(20)).is_err());

        gate.open(&updates_tx);
        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::StateUpdate(_)));
        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::FinishedStateUpdates));
    }
}
