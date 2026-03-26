//! Sparse trie task interface types shared between the engine tree and the payload builder.

use crate::root::ParallelStateRootError;
use alloy_eip7928::BlockAccessList;
use alloy_evm::block::StateChangeSource;
use alloy_primitives::{keccak256, B256};
use reth_trie::{updates::TrieUpdates, HashedPostState, HashedStorage, MultiProofTargetsV2};
use revm_state::EvmState;
use std::sync::Arc;
use tracing::trace;

/// Source of state changes, either from EVM execution or from a Block Access List.
#[derive(Clone, Copy)]
pub enum Source {
    /// State changes from EVM execution.
    Evm(StateChangeSource),
    /// State changes from Block Access List (EIP-7928).
    BlockAccessList,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Evm(source) => source.fmt(f),
            Self::BlockAccessList => f.write_str("BlockAccessList"),
        }
    }
}

impl From<StateChangeSource> for Source {
    fn from(source: StateChangeSource) -> Self {
        Self::Evm(source)
    }
}

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub enum MultiProofMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargetsV2),
    /// New state update from transaction execution with its source
    StateUpdate(Source, EvmState),
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

/// Handle to a background sparse trie computation, passed from the engine to the payload builder.
#[derive(Debug)]
pub struct SparseTrieHandle {
    /// The state root that the cached sparse trie is anchored at.
    ///
    /// Typically matches the parent block's state root. In multi-iteration payload builds,
    /// this may match a previously built payload's state root, allowing the builder to
    /// extend the trie incrementally instead of re-proving from scratch.
    pub cached_trie_state_root: B256,
    /// Sender for streaming state updates and proof targets into the sparse trie pipeline.
    pub updates_tx: crossbeam_channel::Sender<MultiProofMessage>,
    /// Receiver for the final state root result.
    pub state_root_rx:
        std::sync::mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>,
}

/// Converts [`EvmState`] to [`HashedPostState`] by keccak256-hashing addresses and storage slots.
pub fn evm_state_to_hashed_post_state(update: EvmState) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.len());

    for (address, account) in update {
        if account.is_touched() {
            let hashed_address = keccak256(address);
            trace!(target: "trie::parallel::sparse", ?address, ?hashed_address, "Adding account to state update");

            let destroyed = account.is_selfdestructed();
            let info = if destroyed { None } else { Some(account.info.into()) };
            hashed_state.accounts.insert(hashed_address, info);

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
