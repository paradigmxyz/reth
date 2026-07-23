//! State-root task interface types shared between the engine tree and the payload builder.
//!
//! The "state-root task" is the background multiproof and sparse-trie pipeline that computes
//! state roots incrementally while a block executes. This module holds its boundary types:
//! the input messages, the [`StateRootSink`](crate::state_root_task::StateRootSink) and
//! stream views that feed it, and the handles
//! that await its result. The per-block strategy abstraction that decides whether and how the
//! task runs lives in `reth-engine-tree` under `tree::state_root_strategy`.

use crate::error::StateRootTaskError;
use alloy_evm::block::OnStateHook;
use alloy_primitives::{keccak256, map::B256Map, B256};
use reth_trie::{
    updates::TrieUpdates, HashedPostState, HashedStorage, MultiProofTargetsV2, ProofV2Target,
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
    /// Post-block storage roots keyed by hashed account address.
    StorageRoots(B256Map<B256>),
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
    /// Hashed post state produced while computing the state root.
    pub hashed_state: Arc<HashedPostState>,
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
/// Created by the engine's state-root strategy.
#[derive(Debug)]
pub struct StateRootHandle {
    /// The state root that the cached sparse trie is anchored at (parent block's state root).
    cached_trie_state_root: B256,
    /// Best-effort hint capability, taken once by prewarm wiring.
    hint: Option<StateRootHintStream>,
    /// The single authoritative update capability.
    ///
    /// Taken exactly once, either as an execution hook (serial execution) or as a hashed
    /// update stream (parallel BAL streaming), so per block exactly one producer can finish
    /// the update stream. Only producers hold update senders: once the taken capabilities are
    /// dropped or finished, the update channel closes and the task knows producers are done.
    authoritative: Option<StateRootUpdateStream>,
    /// Guard whose drop cancels the state-root task if it is still running.
    cancel_guard: StateRootTaskCancelGuard,
    /// Receiver for the final state root result.
    state_root_rx:
        Option<std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, StateRootTaskError>>>,
    /// Receiver for the hashed post state.
    hashed_state_rx: Option<std::sync::mpsc::Receiver<Arc<HashedPostState>>>,
}

impl StateRootHandle {
    /// Creates a new [`StateRootHandle`].
    pub fn new(
        cached_trie_state_root: B256,
        updates_tx: crossbeam_channel::Sender<StateRootMessage>,
        cancel_guard: StateRootTaskCancelGuard,
        state_root_rx: std::sync::mpsc::Receiver<
            Result<StateRootComputeOutcome, StateRootTaskError>,
        >,
        hashed_state_rx: std::sync::mpsc::Receiver<Arc<HashedPostState>>,
    ) -> Self {
        let sink: Arc<dyn StateRootSink> = Arc::new(SparseTrieStateRootSink::new(updates_tx));
        Self {
            cached_trie_state_root,
            hint: Some(StateRootHintStream::new(Arc::clone(&sink))),
            authoritative: Some(StateRootUpdateStream::new(sink)),
            cancel_guard,
            state_root_rx: Some(state_root_rx),
            hashed_state_rx: Some(hashed_state_rx),
        }
    }

    /// Returns the state root that the cached sparse trie is anchored at.
    pub const fn cached_trie_state_root(&self) -> B256 {
        self.cached_trie_state_root
    }

    /// Takes the best-effort hint capability used by transaction prewarming.
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub const fn take_hint_stream(&mut self) -> StateRootHintStream {
        self.hint.take().expect("hint stream already taken")
    }

    /// Takes the authoritative update capability as an EVM state hook.
    ///
    /// The hook finishes the update stream when dropped. It shares one slot with
    /// [`Self::take_hashed_update_stream`], so only one of the two can exist per block.
    ///
    /// # Panics
    ///
    /// If the authoritative capability was already taken in either form.
    pub fn take_execution_hook(&mut self) -> StateRootUpdateHook {
        self.take_hashed_update_stream().into_state_hook()
    }

    /// Takes the authoritative update capability as a pre-hashed update stream.
    ///
    /// The stream is finished explicitly with [`StateRootUpdateStream::finish`]. It shares
    /// one slot with [`Self::take_execution_hook`], so only one of the two can exist per
    /// block.
    ///
    /// # Panics
    ///
    /// If the authoritative capability was already taken in either form.
    pub const fn take_hashed_update_stream(&mut self) -> StateRootUpdateStream {
        self.authoritative.take().expect("authoritative update capability already taken")
    }

    /// Awaits the state root computation result.
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub fn state_root(&mut self) -> Result<StateRootComputeOutcome, StateRootTaskError> {
        self.state_root_rx
            .take()
            .expect("state_root already taken")
            .recv()
            .map_err(|_| StateRootTaskError::Other("sparse trie task dropped".to_string()))?
    }

    /// Takes the state root receiver for use with custom waiting logic (e.g., timeouts).
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub const fn take_state_root_rx(
        &mut self,
    ) -> std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, StateRootTaskError>> {
        self.state_root_rx.take().expect("state_root already taken")
    }

    /// Takes the hashed state receiver
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub const fn take_hashed_state_rx(
        &mut self,
    ) -> std::sync::mpsc::Receiver<Arc<HashedPostState>> {
        self.hashed_state_rx.take().expect("hashed_state already taken")
    }

    /// Converts this sparse-trie handle into the opaque handle passed to payload builders.
    ///
    /// The payload builder only executes transactions, so the handle carries the execution
    /// hook; the hint capability is dropped here.
    pub fn into_payload_state_root_handle(mut self) -> PayloadStateRootHandle {
        let hook = self.take_execution_hook();
        PayloadStateRootHandle {
            name: "sparse-trie",
            hook: Some(hook),
            cancel_guard: Some(self.cancel_guard),
            state_root_rx: self.state_root_rx.take(),
            hashed_state_rx: self.hashed_state_rx.take(),
        }
    }
}

/// Guard that cancels a state-root task when dropped.
///
/// The task watches the paired receiver in its event loop. No message is ever sent: the guard
/// dropping disconnects the channel, which the task treats as the consumer abandoning the
/// computation (for example on a timeout fallback or when a payload job is dropped unused).
#[derive(Debug)]
pub struct StateRootTaskCancelGuard(#[allow(dead_code)] crossbeam_channel::Sender<()>);

impl StateRootTaskCancelGuard {
    /// Creates a guard and the receiver a task watches for cancellation.
    pub fn channel() -> (Self, crossbeam_channel::Receiver<()>) {
        let (tx, rx) = crossbeam_channel::bounded(0);
        (Self(tx), rx)
    }
}

/// Opaque state-root task handle passed to payload builders.
pub struct PayloadStateRootHandle {
    name: &'static str,
    /// Execution hook that streams per-transaction updates; taken once when building starts.
    hook: Option<StateRootUpdateHook>,
    /// Cancels the backing task when the handle is dropped without consuming the result.
    cancel_guard: Option<StateRootTaskCancelGuard>,
    state_root_rx:
        Option<std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, StateRootTaskError>>>,
    hashed_state_rx: Option<std::sync::mpsc::Receiver<Arc<HashedPostState>>>,
}

impl fmt::Debug for PayloadStateRootHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayloadStateRootHandle")
            .field("name", &self.name)
            .field("has_hook", &self.hook.is_some())
            .field("has_cancel_guard", &self.cancel_guard.is_some())
            .field("has_state_root_rx", &self.state_root_rx.is_some())
            .field("has_hashed_state_rx", &self.hashed_state_rx.is_some())
            .finish()
    }
}

impl PayloadStateRootHandle {
    /// Creates an opaque payload state-root handle.
    ///
    /// Tasks with a drop-to-cancel guard should attach it via the `StateRootHandle`
    /// conversion; handles created here rely on their own task lifecycle.
    pub const fn new(
        name: &'static str,
        hook: Option<StateRootUpdateHook>,
        state_root_rx: std::sync::mpsc::Receiver<
            Result<StateRootComputeOutcome, StateRootTaskError>,
        >,
        hashed_state_rx: Option<std::sync::mpsc::Receiver<Arc<HashedPostState>>>,
    ) -> Self {
        Self { name, hook, cancel_guard: None, state_root_rx: Some(state_root_rx), hashed_state_rx }
    }

    /// Returns the task name used in logs.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Takes the state hook that streams execution updates and finishes the stream on drop.
    ///
    /// # Panics
    ///
    /// If the handle was created without an execution hook, or the hook was already taken.
    pub const fn take_state_hook(&mut self) -> StateRootUpdateHook {
        self.hook.take().expect("payload state root task missing execution hook")
    }

    /// Awaits the state root computation result.
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub fn state_root(&mut self) -> Result<StateRootComputeOutcome, StateRootTaskError> {
        self.state_root_rx
            .take()
            .expect("state_root already taken")
            .recv()
            .map_err(|_| StateRootTaskError::Other("state root task dropped".to_string()))?
    }

    /// Takes the state root receiver for use with custom waiting logic (e.g., timeouts).
    ///
    /// Dropping the handle continues to cancel the backing task if it owns a cancellation guard.
    ///
    /// # Panics
    ///
    /// If called more than once.
    pub const fn take_state_root_rx(
        &mut self,
    ) -> std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, StateRootTaskError>> {
        self.state_root_rx.take().expect("state_root already taken")
    }

    /// Takes the hashed state receiver, if the handle was built with one and it was not taken
    /// yet.
    pub const fn try_take_hashed_state_rx(
        &mut self,
    ) -> Option<std::sync::mpsc::Receiver<Arc<HashedPostState>>> {
        self.hashed_state_rx.take()
    }
}

/// Hashed account and storage keys that a state-root task may want to prefetch.
///
/// Hints are not authoritative. They may be missing, duplicated, stale, or ignored by a task.
/// The conversions from and to proof-target types allocate; that cost is accepted because
/// hints are produced on prewarm workers, off the block-execution thread.
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

/// Authoritative update capability of a state-root stream.
///
/// Exactly one of these exists per state-root task, so exactly one producer can end the
/// update stream: either the EVM state hook made with [`Self::into_state_hook`] (finishes on
/// drop) or a pre-hashed update producer such as BAL streaming (calls [`Self::finish`]). The
/// type is deliberately not `Clone` and finishing consumes it, so a second end-of-stream
/// signal cannot be produced.
///
/// Dropping the stream without calling [`Self::finish`] (for example when a producer dies)
/// deliberately does not finish it: an unfinished stream means the updates are incomplete,
/// and the task must not compute a root from them.
pub struct StateRootUpdateStream {
    inner: Arc<dyn StateRootSink>,
}

impl fmt::Debug for StateRootUpdateStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootUpdateStream").finish_non_exhaustive()
    }
}

impl StateRootUpdateStream {
    /// Creates a new authoritative update stream backed by the given sink.
    pub fn new(inner: Arc<dyn StateRootSink>) -> Self {
        Self { inner }
    }

    /// Emits an authoritative pre-hashed state update.
    pub fn on_hashed_state_update(&self, state: HashedPostState) {
        self.inner.on_hashed_state_update(state);
    }

    /// Finishes the authoritative update stream.
    pub fn finish(self) {
        self.inner.on_updates_finished();
    }

    /// Converts this capability into an EVM state hook that finishes the stream on drop.
    ///
    /// See [`StateRootUpdateHook`] for why the hook finishes on drop while the bare stream
    /// does not, and how a panic during execution is excluded from that.
    pub fn into_state_hook(self) -> StateRootUpdateHook {
        StateRootUpdateHook { inner: self.inner }
    }
}

/// EVM hook that forwards state updates into a [`StateRootSink`].
///
/// Dropping the hook signals the end of the update stream, so the hook is deliberately not
/// `Clone`: a second copy would fire a spurious end-of-stream signal.
///
/// Unlike [`StateRootUpdateStream::finish`], the end of the stream is signaled by drop and
/// not by an explicit call, because the EVM owns the hook until it is dropped and gives it no
/// other end-of-execution signal. A drop during a panic unwind is excluded: execution died
/// mid-block, so the stream stays unfinished and the task reports an error instead of
/// computing a root from incomplete updates. Execution that fails by returning an error still
/// drops the hook normally and finishes the stream; the caller abandons the result in that
/// case, and the stored trie is rejected by the anchor check on the next block.
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
        // A drop during a panic unwind means execution died mid-block. Leave the stream
        // unfinished so the task fails instead of computing a root from partial updates.
        if std::thread::panicking() {
            return;
        }
        self.inner.on_updates_finished();
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
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

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
    fn state_root_capabilities_forward_to_sink() {
        let sink = Arc::new(CountingSink::default());

        let hint_stream = StateRootHintStream::new(sink.clone());
        let mut storages = B256Map::default();
        storages.insert(B256::repeat_byte(0x02), vec![B256::repeat_byte(0x03)]);
        hint_stream
            .on_access_hint(StateAccessHint { accounts: vec![B256::repeat_byte(0x01)], storages });

        let updates = StateRootUpdateStream::new(sink.clone());
        updates.on_hashed_state_update(HashedPostState::default());
        updates.finish();

        {
            let mut hook = StateRootUpdateStream::new(sink.clone()).into_state_hook();
            hook.on_state(EvmState::default());
        }

        assert_eq!(sink.access_hints.load(Ordering::Relaxed), 1);
        assert_eq!(sink.state_updates.load(Ordering::Relaxed), 1);
        assert_eq!(sink.hashed_state_updates.load(Ordering::Relaxed), 1);
        assert_eq!(sink.finished_updates.load(Ordering::Relaxed), 2);
    }

    /// A hook dropped by a panic unwind must not finish the stream: the updates are
    /// incomplete, and a finish marker would make the task compute a root from them.
    #[test]
    fn hook_dropped_during_panic_does_not_finish_stream() {
        let sink = Arc::new(CountingSink::default());
        let hook = StateRootUpdateStream::new(sink.clone()).into_state_hook();

        let result = std::thread::spawn(move || {
            let _hook = hook;
            panic!("execution died mid-block");
        })
        .join();

        assert!(result.is_err());
        assert_eq!(sink.finished_updates.load(Ordering::Relaxed), 0);
    }

    /// The authoritative capability is a single slot: taking it as a hook and then again as
    /// a hashed update stream (or in any other combination) must panic.
    #[test]
    #[should_panic(expected = "authoritative update capability already taken")]
    fn authoritative_capability_can_only_be_taken_once() {
        let (updates_tx, _updates_rx) = crossbeam_channel::unbounded();
        let (cancel_guard, _cancel_rx) = StateRootTaskCancelGuard::channel();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let mut handle = StateRootHandle::new(
            B256::ZERO,
            updates_tx,
            cancel_guard,
            state_root_rx,
            hashed_state_rx,
        );

        let _hook = handle.take_execution_hook();
        let _ = handle.take_hashed_update_stream();
    }

    /// Lifecycle of the opaque handle a strategy hands to the payload builder: the execution
    /// hook streams updates into the sink and signals completion on drop, the hashed-state
    /// receiver can be taken exactly once, and the outcome arrives through the state-root
    /// channel.
    #[test]
    fn payload_state_root_handle_lifecycle() {
        let sink = Arc::new(CountingSink::default());
        let hook = StateRootUpdateStream::new(sink.clone()).into_state_hook();

        let (state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let mut handle =
            PayloadStateRootHandle::new("test", Some(hook), state_root_rx, Some(hashed_state_rx));

        assert_eq!(handle.name(), "test");

        {
            let mut hook = handle.take_state_hook();
            hook.on_state(EvmState::default());
        }
        assert_eq!(sink.state_updates.load(Ordering::Relaxed), 1);
        assert_eq!(sink.finished_updates.load(Ordering::Relaxed), 1);

        hashed_state_tx.send(Arc::new(HashedPostState::default())).unwrap();
        let rx = handle.try_take_hashed_state_rx().expect("first take returns the receiver");
        assert!(rx.recv().is_ok());
        assert!(handle.try_take_hashed_state_rx().is_none(), "second take returns None");

        state_root_tx
            .send(Ok(StateRootComputeOutcome {
                state_root: B256::repeat_byte(0x42),
                trie_updates: Arc::new(TrieUpdates::default()),
                hashed_state: Arc::new(HashedPostState::default()),
                #[cfg(feature = "trie-debug")]
                debug_recorders: Vec::new(),
            }))
            .unwrap();
        let outcome = handle.state_root().expect("outcome is delivered");
        assert_eq!(outcome.state_root, B256::repeat_byte(0x42));
    }

    #[test]
    #[should_panic(expected = "state_root already taken")]
    fn payload_state_root_receiver_can_only_be_taken_once() {
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let mut handle = PayloadStateRootHandle::new("test", None, state_root_rx, None);

        let _state_root_rx = handle.take_state_root_rx();
        let _ = handle.take_state_root_rx();
    }

    #[test]
    fn payload_state_root_receiver_retains_cancellation() {
        let (updates_tx, _updates_rx) = crossbeam_channel::unbounded();
        let (cancel_guard, cancel_rx) = StateRootTaskCancelGuard::channel();
        let (_state_root_tx, state_root_rx) = std::sync::mpsc::channel();
        let (_hashed_state_tx, hashed_state_rx) = std::sync::mpsc::channel();
        let mut handle = StateRootHandle::new(
            B256::ZERO,
            updates_tx,
            cancel_guard,
            state_root_rx,
            hashed_state_rx,
        )
        .into_payload_state_root_handle();

        let state_root_rx = handle.take_state_root_rx();
        assert!(matches!(
            state_root_rx.recv_timeout(Duration::ZERO),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(cancel_rx.try_recv(), Err(crossbeam_channel::TryRecvError::Empty)));

        drop(handle);
        assert!(matches!(
            cancel_rx.recv_timeout(Duration::from_secs(1)),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected)
        ));
    }
}
