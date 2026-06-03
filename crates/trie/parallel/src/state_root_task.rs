//! State root task interface types shared between the engine tree and the payload builder.

use crate::root::ParallelStateRootError;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::{keccak256, B256};
use derive_more::derive::Deref;
use reth_trie::{updates::TrieUpdates, HashedPostState, HashedStorage, MultiProofTargetsV2};
use revm_state::EvmState;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::trace;

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub enum StateRootMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargetsV2),
    /// New state update from transaction execution with its source
    StateUpdate(EvmState),
    /// Pre-hashed state update from BAL conversion that can be applied directly without proofs.
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
    /// Shared flag set while this handle is waiting for its parent sparse trie to become available.
    deferred_parent_pending: Option<Arc<AtomicBool>>,
    /// Channel for streaming state updates and proof targets into the sparse trie pipeline.
    updates_tx: crossbeam_channel::Sender<StateRootMessage>,
    /// Receiver for the final state root result.
    state_root_rx:
        Option<std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>>,
    /// Receiver for the hashed post state.
    hashed_state_rx: Option<std::sync::mpsc::Receiver<HashedPostState>>,
    /// Set when the payload-builder side drops this handle before consuming sparse-trie outputs.
    consumer_cancelled: Arc<AtomicBool>,
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
            deferred_parent_pending: None,
            updates_tx,
            state_root_rx: Some(state_root_rx),
            hashed_state_rx: Some(hashed_state_rx),
            consumer_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a new deferred [`StateRootHandle`].
    ///
    /// A deferred handle queues payload-builder updates until the parent block has been validated
    /// and an active sparse-trie task can be rooted at the parent's post-state.
    pub fn new_deferred(
        cached_trie_state_root: B256,
        updates_tx: crossbeam_channel::Sender<StateRootMessage>,
        state_root_rx: std::sync::mpsc::Receiver<
            Result<StateRootComputeOutcome, ParallelStateRootError>,
        >,
        hashed_state_rx: std::sync::mpsc::Receiver<HashedPostState>,
        deferred_parent_pending: Arc<AtomicBool>,
    ) -> Self {
        Self {
            cached_trie_state_root,
            deferred_parent_pending: Some(deferred_parent_pending),
            updates_tx,
            state_root_rx: Some(state_root_rx),
            hashed_state_rx: Some(hashed_state_rx),
            consumer_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns the state root that the cached sparse trie is anchored at.
    pub const fn cached_trie_state_root(&self) -> B256 {
        self.cached_trie_state_root
    }

    /// Returns true if this handle is waiting for parent validation before sparse-trie work can run.
    pub fn is_deferred(&self) -> bool {
        self.deferred_parent_pending.is_some()
    }

    /// Returns true if this handle is still waiting for parent validation.
    pub fn is_deferred_parent_pending(&self) -> bool {
        self.deferred_parent_pending
            .as_ref()
            .is_some_and(|pending| pending.load(Ordering::Acquire))
    }

    /// Returns a flag that is set once this handle is dropped by its consumer.
    pub fn consumer_cancelled(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.consumer_cancelled)
    }

    /// Returns a reference to the updates sender channel.
    pub const fn updates_tx(&self) -> &crossbeam_channel::Sender<StateRootMessage> {
        &self.updates_tx
    }

    /// Returns a state hook that streams state updates to the background state root task.
    ///
    /// The hook must be dropped after execution completes to signal the end of state updates.
    pub fn state_hook(&self) -> impl alloy_evm::block::OnStateHook {
        let sender = StateHookSender::new(self.updates_tx.clone());

        move |state: &EvmState| {
            let _ = sender.send(StateRootMessage::StateUpdate(state.clone()));
        }
    }

    /// Returns a state hook that flattens updates until `is_released` returns true.
    ///
    /// A deferred parent also keeps updates flattened. Once released, any flattened updates are
    /// flushed and subsequent updates use the same stream as [`Self::state_hook`]. If the hook is
    /// dropped before release, the flattened updates are discarded.
    pub fn state_hook_flatten_until<F>(
        &self,
        is_released: F,
    ) -> impl alloy_evm::block::OnStateHook
    where
        F: Fn() -> bool + Send + 'static,
    {
        let mut hook = DeferredFlatteningStateHook::new(
            self.updates_tx.clone(),
            self.deferred_parent_pending.clone(),
            is_released,
        );

        move |state: &EvmState| hook.on_state(state)
    }

    /// Returns a state hook that flattens updates only while a deferred parent is pending.
    ///
    /// Once parent validation fulfills the handle, any flattened updates are flushed and subsequent
    /// updates use the same stream as [`Self::state_hook`].
    pub fn state_hook_flatten_while_deferred(&self) -> impl alloy_evm::block::OnStateHook {
        self.state_hook_flatten_until(|| true)
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
}

impl Drop for StateHookSender {
    fn drop(&mut self) {
        // Send completion signal when the sender is dropped
        let _ = self.0.send(StateRootMessage::FinishedStateUpdates);
    }
}

struct DeferredFlatteningStateHook<F>
where
    F: Fn() -> bool,
{
    sender: StateHookSender,
    deferred_parent_pending: Option<Arc<AtomicBool>>,
    is_released: F,
    flattened_state: HashedPostState,
}

impl<F> DeferredFlatteningStateHook<F>
where
    F: Fn() -> bool,
{
    fn new(
        sender: crossbeam_channel::Sender<StateRootMessage>,
        deferred_parent_pending: Option<Arc<AtomicBool>>,
        is_released: F,
    ) -> Self {
        Self {
            sender: StateHookSender::new(sender),
            deferred_parent_pending,
            is_released,
            flattened_state: HashedPostState::default(),
        }
    }

    fn on_state(&mut self, state: &EvmState) {
        if self
            .deferred_parent_pending
            .as_ref()
            .is_some_and(|pending| pending.load(Ordering::Acquire))
            || !(self.is_released)()
        {
            self.flattened_state.extend(evm_state_to_hashed_post_state(state.clone()));
            return
        }

        self.flush_flattened_state();
        let _ = self.sender.send(StateRootMessage::StateUpdate(state.clone()));
    }
}

impl<F> DeferredFlatteningStateHook<F>
where
    F: Fn() -> bool,
{
    fn flush_flattened_state(&mut self) {
        if self.flattened_state.is_empty() {
            return
        }

        let flattened_state = core::mem::take(&mut self.flattened_state);
        let _ = self.sender.send(StateRootMessage::HashedStateUpdate(flattened_state));
    }
}

impl<F> Drop for DeferredFlatteningStateHook<F>
where
    F: Fn() -> bool,
{
    fn drop(&mut self) {
        if (self.is_released)() {
            self.flush_flattened_state();
        }
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
    fn state_root_handle_tracks_deferred_parent() {
        let (updates_tx, _updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();

        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx, state_root_rx, hashed_state_rx);
        assert!(!handle.is_deferred());

        let (updates_tx, _updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();

        let pending = Arc::new(AtomicBool::new(true));
        let handle = StateRootHandle::new_deferred(
            B256::ZERO,
            updates_tx,
            state_root_rx,
            hashed_state_rx,
            pending,
        );
        assert!(handle.is_deferred());
        assert!(handle.is_deferred_parent_pending());
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
    fn state_hook_flatten_until_release_gates_updates() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx, state_root_rx, hashed_state_rx);

        let released = Arc::new(AtomicBool::new(false));
        let mut hook = handle.state_hook_flatten_until({
            let released = released.clone();
            move || released.load(Ordering::Acquire)
        });

        hook.on_state(&EvmState::default());
        assert!(updates_rx.try_recv().is_err());

        released.store(true, Ordering::Release);
        hook.on_state(&EvmState::default());
        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::StateUpdate(_)));

        drop(hook);
        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::FinishedStateUpdates));
    }

    #[test]
    fn state_hook_flatten_until_discards_updates_when_dropped_before_release() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let handle =
            StateRootHandle::new(B256::ZERO, updates_tx, state_root_rx, hashed_state_rx);

        let mut hook = handle.state_hook_flatten_until(|| false);
        hook.on_state(&EvmState::default());
        drop(hook);

        assert!(matches!(updates_rx.recv().unwrap(), StateRootMessage::FinishedStateUpdates));
        assert!(updates_rx.try_recv().is_err());
    }
}
