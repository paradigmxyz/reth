//! State root task interface types shared between the engine tree and the payload builder.

use crate::root::ParallelStateRootError;
use alloy_evm::block::OnStateHook;
use alloy_primitives::{keccak256, map::B256Map, B256};
use derive_more::derive::Deref;
use reth_trie::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdates, HashedPostState, HashedStorage,
    MultiProofTargetsV2, ProofV2Target,
};
use revm::state::EvmState;
use std::{fmt, sync::Arc};
use tracing::trace;

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub enum StateRootMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargetsV2),
    /// New state update from transaction execution.
    StateUpdate(EvmState),
    /// Pre-hashed state update from BAL conversion that can be applied directly without proofs.
    HashedStateUpdate(HashedPostState),
    /// Signals state update stream end.
    ///
    /// This is triggered by block execution, indicating that no additional state updates are
    /// expected.
    FinishedStateUpdates,
}

/// Hashed account and storage keys that a state-root task may want to prefetch.
///
/// Hints are not authoritative. They may be missing, duplicated, stale, or ignored by a task.
#[derive(Debug, Clone, Default)]
pub struct StateAccessHint {
    /// Hashed account keys that may be touched later in the block.
    pub accounts: Vec<B256>,
    /// Hashed storage keys keyed by hashed account.
    pub storages: B256Map<Vec<B256>>,
}

impl From<MultiProofTargetsV2> for StateAccessHint {
    fn from(targets: MultiProofTargetsV2) -> Self {
        Self {
            accounts: targets.account_targets.into_iter().map(|target| target.key()).collect(),
            storages: targets
                .storage_targets
                .into_iter()
                .map(|(account, slots)| {
                    (account, slots.into_iter().map(|target| target.key()).collect())
                })
                .collect(),
        }
    }
}

impl From<StateAccessHint> for MultiProofTargetsV2 {
    fn from(hint: StateAccessHint) -> Self {
        Self {
            account_targets: hint.accounts.into_iter().map(ProofV2Target::from).collect(),
            storage_targets: hint
                .storages
                .into_iter()
                .map(|(account, slots)| {
                    (account, slots.into_iter().map(ProofV2Target::from).collect())
                })
                .collect(),
        }
    }
}

/// Semantic update stream consumed by state-root tasks.
pub trait StateRootSink: Send + Sync + 'static {
    /// Best-effort access hint from transaction prewarming.
    fn on_access_hint(&self, _hint: StateAccessHint) {}

    /// Authoritative state update from normal block execution.
    fn on_state_update(&self, state: EvmState);

    /// Authoritative pre-hashed state update, currently used by BAL streaming.
    fn on_hashed_state_update(&self, state: HashedPostState);

    /// Signals that no more authoritative state updates are expected.
    fn on_updates_finished(&self);
}

/// Hint-only view of a state-root stream.
#[derive(Clone)]
pub struct StateRootHintStream {
    inner: Arc<dyn StateRootSink>,
}

impl fmt::Debug for StateRootHintStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootHintStream").finish_non_exhaustive()
    }
}

impl StateRootHintStream {
    /// Creates a new hint stream view.
    pub fn new(inner: Arc<dyn StateRootSink>) -> Self {
        Self { inner }
    }

    /// Emits a best-effort access hint.
    pub fn on_access_hint(&self, hint: StateAccessHint) {
        self.inner.on_access_hint(hint);
    }
}

/// Pre-hashed authoritative update view of a state-root stream.
#[derive(Clone)]
pub struct StateRootHashedUpdateStream {
    inner: Arc<dyn StateRootSink>,
}

impl fmt::Debug for StateRootHashedUpdateStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootHashedUpdateStream").finish_non_exhaustive()
    }
}

impl StateRootHashedUpdateStream {
    /// Creates a new hashed update stream view.
    pub fn new(inner: Arc<dyn StateRootSink>) -> Self {
        Self { inner }
    }

    /// Emits an authoritative pre-hashed state update.
    pub fn on_hashed_state_update(&self, state: HashedPostState) {
        self.inner.on_hashed_state_update(state);
    }

    /// Finishes the authoritative update stream.
    pub fn on_updates_finished(&self) {
        self.inner.on_updates_finished();
    }
}

/// Normal execution view of a state-root stream.
#[derive(Clone)]
pub struct StateRootExecutionStream {
    inner: Arc<dyn StateRootSink>,
}

impl fmt::Debug for StateRootExecutionStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootExecutionStream").finish_non_exhaustive()
    }
}

impl StateRootExecutionStream {
    /// Creates a new execution stream view.
    pub fn new(inner: Arc<dyn StateRootSink>) -> Self {
        Self { inner }
    }

    /// Returns an EVM state hook that finishes the stream when dropped.
    pub fn state_hook(&self) -> StateRootUpdateHook {
        StateRootUpdateHook { inner: Arc::clone(&self.inner) }
    }
}

/// State-root streams exposed to execution and prewarm code.
#[derive(Clone, Default)]
pub struct StateRootStreams {
    hint: Option<StateRootHintStream>,
    hashed_updates: Option<StateRootHashedUpdateStream>,
    execution: Option<StateRootExecutionStream>,
}

impl fmt::Debug for StateRootStreams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootStreams")
            .field("has_hint_stream", &self.hint.is_some())
            .field("has_hashed_update_stream", &self.hashed_updates.is_some())
            .field("has_execution_stream", &self.execution.is_some())
            .finish()
    }
}

impl StateRootStreams {
    /// Creates stream views backed by one sink.
    pub fn from_sink(inner: Arc<dyn StateRootSink>, install_execution_hook: bool) -> Self {
        Self {
            hint: Some(StateRootHintStream::new(Arc::clone(&inner))),
            hashed_updates: Some(StateRootHashedUpdateStream::new(Arc::clone(&inner))),
            execution: install_execution_hook.then(|| StateRootExecutionStream::new(inner)),
        }
    }

    /// Creates a stream set with no state-root task attached.
    pub const fn empty() -> Self {
        Self { hint: None, hashed_updates: None, execution: None }
    }

    /// Returns the hint-only stream.
    pub fn hint_stream(&self) -> Option<StateRootHintStream> {
        self.hint.clone()
    }

    /// Returns the pre-hashed update stream.
    pub fn hashed_update_stream(&self) -> Option<StateRootHashedUpdateStream> {
        self.hashed_updates.clone()
    }

    /// Returns true if no stream views are installed.
    pub const fn is_empty(&self) -> bool {
        self.hint.is_none() && self.hashed_updates.is_none() && self.execution.is_none()
    }

    /// Takes the execution stream.
    pub fn take_execution_stream(&mut self) -> Option<StateRootExecutionStream> {
        self.execution.take()
    }
}

/// EVM hook that forwards state updates into a [`StateRootSink`].
#[derive(Clone)]
pub struct StateRootUpdateHook {
    inner: Arc<dyn StateRootSink>,
}

impl fmt::Debug for StateRootUpdateHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootUpdateHook").finish_non_exhaustive()
    }
}

impl OnStateHook for StateRootUpdateHook {
    fn on_state(&mut self, state: EvmState) {
        self.inner.on_state_update(state);
    }
}

impl Drop for StateRootUpdateHook {
    fn drop(&mut self) {
        self.inner.on_updates_finished();
    }
}

/// Outcome of the state root computation, including the state root itself with
/// the trie updates.
#[derive(Debug, Clone)]
pub struct StateRootComputeOutcome {
    /// The state root.
    pub state_root: B256,
    /// The trie updates.
    pub trie_updates: Arc<TrieUpdates>,
    /// Changed trie node base paths retained while computing the root.
    pub changed_paths: Option<Arc<TriePrefixSetsMut>>,
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

    /// Returns semantic stream views backed by this sparse trie task.
    pub fn streams(&self, install_execution_hook: bool) -> StateRootStreams {
        StateRootStreams::from_sink(
            Arc::new(SparseTrieStateRootSink::new(self.updates_tx.clone())),
            install_execution_hook,
        )
    }

    /// Returns a state hook that streams state updates to the background state root task.
    ///
    /// The hook must be dropped after execution completes to signal the end of state updates.
    pub fn state_hook(&self) -> impl OnStateHook {
        StateRootExecutionStream::new(Arc::new(SparseTrieStateRootSink::new(
            self.updates_tx.clone(),
        )))
        .state_hook()
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

    /// Converts this sparse-trie handle into the opaque handle passed to payload builders.
    pub fn into_payload_state_root_handle(mut self) -> PayloadStateRootHandle {
        let streams = self.streams(true);
        PayloadStateRootHandle {
            name: "sparse-trie",
            streams,
            state_root_rx: self.state_root_rx.take(),
            hashed_state_rx: self.hashed_state_rx.take(),
        }
    }
}

/// Opaque state-root task handle passed to payload builders.
pub struct PayloadStateRootHandle {
    name: &'static str,
    streams: StateRootStreams,
    state_root_rx:
        Option<std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>>,
    hashed_state_rx: Option<std::sync::mpsc::Receiver<HashedPostState>>,
}

impl fmt::Debug for PayloadStateRootHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayloadStateRootHandle")
            .field("name", &self.name)
            .field("streams", &self.streams)
            .field("has_state_root_rx", &self.state_root_rx.is_some())
            .field("has_hashed_state_rx", &self.hashed_state_rx.is_some())
            .finish()
    }
}

impl PayloadStateRootHandle {
    /// Creates an opaque payload state-root handle.
    pub const fn new(
        name: &'static str,
        streams: StateRootStreams,
        state_root_rx: std::sync::mpsc::Receiver<
            Result<StateRootComputeOutcome, ParallelStateRootError>,
        >,
        hashed_state_rx: Option<std::sync::mpsc::Receiver<HashedPostState>>,
    ) -> Self {
        Self { name, streams, state_root_rx: Some(state_root_rx), hashed_state_rx }
    }

    /// Returns the task name used in logs.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns a state hook that streams execution updates and finishes on drop.
    ///
    /// # Panics
    ///
    /// If the task was created without an execution stream.
    pub fn state_hook(&self) -> impl OnStateHook {
        self.streams
            .execution
            .as_ref()
            .expect("payload state root task missing execution stream")
            .state_hook()
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
            .map_err(|_| ParallelStateRootError::Other("state root task dropped".to_string()))?
    }

    /// Takes the hashed state receiver.
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub fn take_hashed_state_rx(&mut self) -> std::sync::mpsc::Receiver<HashedPostState> {
        self.hashed_state_rx.take().expect("hashed_state already taken")
    }

    /// Takes the hashed state receiver, if the task produces one.
    pub fn try_take_hashed_state_rx(
        &mut self,
    ) -> Option<std::sync::mpsc::Receiver<HashedPostState>> {
        self.hashed_state_rx.take()
    }
}

#[derive(Debug, Clone)]
struct SparseTrieStateRootSink {
    sender: crossbeam_channel::Sender<StateRootMessage>,
}

impl SparseTrieStateRootSink {
    const fn new(sender: crossbeam_channel::Sender<StateRootMessage>) -> Self {
        Self { sender }
    }
}

impl StateRootSink for SparseTrieStateRootSink {
    fn on_access_hint(&self, hint: StateAccessHint) {
        let _ = self.sender.send(StateRootMessage::PrefetchProofs(hint.into()));
    }

    fn on_state_update(&self, state: EvmState) {
        let _ = self.sender.send(StateRootMessage::StateUpdate(state));
    }

    fn on_hashed_state_update(&self, state: HashedPostState) {
        let _ = self.sender.send(StateRootMessage::HashedStateUpdate(state));
    }

    fn on_updates_finished(&self) {
        let _ = self.sender.send(StateRootMessage::FinishedStateUpdates);
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct CountingSink {
        access_hints: AtomicUsize,
        state_updates: AtomicUsize,
        hashed_state_updates: AtomicUsize,
        finished_updates: AtomicUsize,
    }

    impl StateRootSink for CountingSink {
        fn on_access_hint(&self, hint: StateAccessHint) {
            assert_eq!(hint.accounts, vec![B256::repeat_byte(0x01)]);
            assert_eq!(
                hint.storages.get(&B256::repeat_byte(0x02)),
                Some(&vec![B256::repeat_byte(0x03)])
            );
            self.access_hints.fetch_add(1, Ordering::Relaxed);
        }

        fn on_state_update(&self, state: EvmState) {
            assert!(state.is_empty());
            self.state_updates.fetch_add(1, Ordering::Relaxed);
        }

        fn on_hashed_state_update(&self, state: HashedPostState) {
            assert!(state.accounts.is_empty());
            assert!(state.storages.is_empty());
            self.hashed_state_updates.fetch_add(1, Ordering::Relaxed);
        }

        fn on_updates_finished(&self) {
            self.finished_updates.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn state_access_hint_converts_to_sparse_targets() {
        let account = B256::repeat_byte(0x01);
        let storage_account = B256::repeat_byte(0x02);
        let storage_slot = B256::repeat_byte(0x03);

        let mut storages = B256Map::default();
        storages.insert(storage_account, vec![storage_slot]);
        let hint = StateAccessHint { accounts: vec![account], storages };

        let targets = MultiProofTargetsV2::from(hint);
        assert_eq!(targets.account_targets.len(), 1);
        assert_eq!(targets.account_targets[0].key(), account);
        assert_eq!(targets.storage_targets.len(), 1);
        assert_eq!(targets.storage_targets[&storage_account].len(), 1);
        assert_eq!(targets.storage_targets[&storage_account][0].key(), storage_slot);

        let hint = StateAccessHint::from(targets);
        assert_eq!(hint.accounts, vec![account]);
        assert_eq!(hint.storages.len(), 1);
        assert_eq!(hint.storages[&storage_account], vec![storage_slot]);
    }

    #[test]
    fn state_root_streams_forward_to_sink() {
        let sink = Arc::new(CountingSink::default());
        let mut streams = StateRootStreams::from_sink(sink.clone(), true);

        let mut storages = B256Map::default();
        storages.insert(B256::repeat_byte(0x02), vec![B256::repeat_byte(0x03)]);
        streams
            .hint_stream()
            .expect("hint stream")
            .on_access_hint(StateAccessHint { accounts: vec![B256::repeat_byte(0x01)], storages });

        let hashed_updates = streams.hashed_update_stream().expect("hashed update stream");
        hashed_updates.on_hashed_state_update(HashedPostState::default());
        hashed_updates.on_updates_finished();

        let execution_stream = streams.take_execution_stream().expect("execution stream");
        assert!(streams.take_execution_stream().is_none());
        {
            let mut hook = execution_stream.state_hook();
            hook.on_state(EvmState::default());
        }

        assert_eq!(sink.access_hints.load(Ordering::Relaxed), 1);
        assert_eq!(sink.state_updates.load(Ordering::Relaxed), 1);
        assert_eq!(sink.hashed_state_updates.load(Ordering::Relaxed), 1);
        assert_eq!(sink.finished_updates.load(Ordering::Relaxed), 2);
    }
}
