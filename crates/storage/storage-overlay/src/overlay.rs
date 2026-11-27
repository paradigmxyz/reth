//! Computes trie and execution overlays from an in-memory block chain.

use crate::manager_metrics::{ExecutionOverlayMetrics, StateTrieOverlayMetrics};
use alloy_eips::BlockNumHash;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, U256Map},
    Address, B256, U256,
};
use evm2::{bytecode::Bytecode, evm::AccountInfo};
use reth_chain_state::{BlockState, ExecutedBlock, InMemoryState};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use reth_trie_common::bundle_state::BundleState;
use std::{fmt, sync::Arc, time::Instant};
use tracing::{debug, trace};

#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;

/// Execution state required to initialize an overlay state provider.
///
/// Account entries preserve known non-existence, while storage and code entries contain only data
/// explicitly observed during execution.
#[derive(Clone, Debug, Default)]
pub struct ExecutionOverlay {
    /// In-memory block hashes in ascending block-number order.
    block_hashes: Vec<BlockNumHash>,
    /// Account state by address.
    accounts: AddressMap<Option<AccountInfo>>,
    /// Storage values by address and slot.
    storage: AddressMap<U256Map<U256>>,
    /// Accounts whose storage was wiped during execution.
    ///
    /// An absent slot for one of these accounts is known to be zero, rather than falling back to
    /// the durable database state.
    storage_wipes: AddressSet,
    /// Bytecode by code hash.
    code_hashes: B256Map<Bytecode>,
}

impl ExecutionOverlay {
    /// Returns the in-memory block hashes in ascending block-number order.
    pub const fn block_hashes(&self) -> &[BlockNumHash] {
        self.block_hashes.as_slice()
    }

    /// Returns the account state by address.
    pub const fn accounts(&self) -> &AddressMap<Option<AccountInfo>> {
        &self.accounts
    }

    /// Returns the storage values by address and slot.
    pub const fn storage(&self) -> &AddressMap<U256Map<U256>> {
        &self.storage
    }

    /// Returns an explicitly observed storage value, or zero when the account's storage was
    /// wiped.
    pub(crate) fn storage_value(&self, address: Address, slot: U256) -> Option<U256> {
        self.storage
            .get(&address)
            .and_then(|storage| storage.get(&slot))
            .copied()
            .or_else(|| self.storage_wipes.contains(&address).then_some(U256::ZERO))
    }

    /// Returns the bytecode by code hash.
    pub const fn code_hashes(&self) -> &B256Map<Bytecode> {
        &self.code_hashes
    }

    #[cfg(test)]
    pub(crate) const fn block_hashes_mut(&mut self) -> &mut Vec<BlockNumHash> {
        &mut self.block_hashes
    }

    #[cfg(test)]
    pub(crate) const fn accounts_mut(&mut self) -> &mut AddressMap<Option<AccountInfo>> {
        &mut self.accounts
    }

    #[cfg(test)]
    pub(crate) const fn storage_mut(&mut self) -> &mut AddressMap<U256Map<U256>> {
        &mut self.storage
    }

    #[cfg(test)]
    pub(crate) const fn code_hashes_mut(&mut self) -> &mut B256Map<Bytecode> {
        &mut self.code_hashes
    }

    /// Extends this overlay with the execution state of a later block.
    pub(crate) fn extend_block<N: NodePrimitives>(&mut self, block: &ExecutedBlock<N>) {
        self.block_hashes.push(block.recovered_block().num_hash());
        self.extend_state(&block.execution_output.state);
    }

    /// Extends this overlay with a later bundle state.
    fn extend_state(&mut self, state: &BundleState) {
        let (accounts, storage, storage_wipes, code_hashes) =
            (&mut self.accounts, &mut self.storage, &mut self.storage_wipes, &mut self.code_hashes);

        #[allow(unused_mut)]
        let mut extend_accounts_and_storage = || {
            for (address, account) in state.state() {
                if account.info != account.original_info {
                    accounts.insert(*address, account.info.clone());
                }
                if account.was_destroyed() {
                    storage_wipes.insert(*address);
                    storage.remove(address);
                }
                if account.storage.is_empty() {
                    continue
                }
                let account_storage = storage.entry(*address).or_default();
                for (slot, value) in &account.storage {
                    account_storage.insert(*slot, value.present_value);
                }
            }
        };
        #[allow(unused_mut)]
        let mut extend_code_hashes = || {
            code_hashes.extend(state.contracts.iter().map(|(hash, code)| (*hash, code.clone())));
        };

        #[cfg(feature = "rayon")]
        rayon::join(extend_accounts_and_storage, extend_code_hashes);

        #[cfg(not(feature = "rayon"))]
        {
            extend_accounts_and_storage();
            extend_code_hashes();
        }
    }
}

/// Errors resolving a state overlay against an in-memory snapshot and database view.
#[derive(Debug, PartialEq, Eq)]
pub enum OverlayError {
    /// The tracked chain does not reach the required anchor.
    UnreachableAnchor {
        /// Hash of the tracked tip.
        tip_hash: B256,
        /// Hash of the required anchor.
        anchor_hash: B256,
    },
    /// The requested parent was not tracked in the paired view.
    MissingParent(B256),
    /// The database has no Finish checkpoint.
    MissingFinishCheckpoint,
    /// Finish is ahead of the durable trie, but its forward updates are not tracked.
    MissingFinishBlock {
        /// The Finish frontier.
        finish: BlockNumHash,
        /// The durable trie frontier.
        trie_frontier: BlockNumHash,
    },
    /// The in-memory chain does not reach the durable trie.
    AnchorAfterTrieFrontier {
        /// The oldest anchor reachable in the paired view.
        anchor: BlockNumHash,
        /// The durable trie frontier.
        trie_frontier: BlockNumHash,
    },
    /// Computing the Finish overlay would require reverts, which are unavailable during
    /// changeset reconstruction.
    UnexpectedReverts {
        /// The requested parent hash.
        parent_hash: B256,
        /// The Finish frontier.
        finish: BlockNumHash,
        /// The anchor that requires reverts.
        anchor: BlockNumHash,
    },
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreachableAnchor { tip_hash, anchor_hash } => write!(
                f, "in-memory chain ending at {tip_hash} does not reach overlay anchor {anchor_hash}"
            ),
            Self::MissingParent(parent_hash) => write!(
                f, "overlay parent {parent_hash} is missing from the in-memory snapshot"
            ),
            Self::MissingFinishCheckpoint => f.write_str("database is missing the Finish checkpoint"),
            Self::MissingFinishBlock { finish, trie_frontier } => write!(
                f, "Finish {finish:?} is missing from the in-memory snapshot at trie frontier {trie_frontier:?}"
            ),
            Self::AnchorAfterTrieFrontier { anchor, trie_frontier } => write!(
                f, "overlay anchor {anchor:?} is after partial state trie frontier {trie_frontier:?}; forward trie updates are missing from the in-memory snapshot"
            ),
            Self::UnexpectedReverts { parent_hash, finish, anchor } => write!(
                f, "reverts are disabled, but overlay for parent {parent_hash} requires reverting Finish {finish:?} to anchor {anchor:?}"
            ),
        }
    }
}

impl std::error::Error for OverlayError {}

pub(crate) enum ComputeOverlayInput<N: NodePrimitives, T> {
    ExtendCached { block: ExecutedBlock<N>, parent_input: Arc<T> },
    MergeBlocks(Vec<ExecutedBlock<N>>),
}

pub(crate) fn blocks_from_parent_state<N: NodePrimitives>(
    parent_state: &BlockState<N>,
    anchor_hash: B256,
) -> Result<Vec<ExecutedBlock<N>>, OverlayError> {
    let tip_hash = parent_state.hash();
    let mut hash = tip_hash;
    let mut blocks = Vec::new();
    for state in parent_state.chain() {
        let block = state.block();
        if block.recovered_block().hash() != hash {
            return Err(OverlayError::UnreachableAnchor { tip_hash, anchor_hash })
        }
        hash = block.recovered_block().parent_hash();
        blocks.push(block);
        if hash == anchor_hash {
            return Ok(blocks)
        }
    }
    Err(OverlayError::UnreachableAnchor { tip_hash, anchor_hash })
}

/// Returns whether the tip is tracked and its live chain reaches the anchor.
pub(crate) fn live_state_reaches<N: NodePrimitives>(
    in_memory_state: &InMemoryState<N>,
    tip_hash: B256,
    anchor_hash: B256,
) -> bool {
    in_memory_state
        .state_by_hash(tip_hash)
        .is_some_and(|state| state.chain_hashes().any(|hash| hash == anchor_hash))
}

/// Runs the computation on the given worker pool, or on the calling thread without one.
pub(crate) fn compute_with_worker_pool<N, T, M>(
    input: ComputeOverlayInput<N, T>,
    anchor_hash: B256,
    metrics: &M,
    #[cfg(feature = "rayon")] worker_pool: Option<&WorkerPool>,
    span: tracing::Span,
    compute: impl FnOnce(ComputeOverlayInput<N, T>, B256, &M) -> T + Send + 'static,
) -> T
where
    N: NodePrimitives,
    T: Send + Sync + 'static,
    M: Clone + Send + 'static,
{
    #[cfg(feature = "rayon")]
    if let Some(worker_pool) = worker_pool {
        let metrics = metrics.clone();
        return worker_pool.spawn_and_wait(move || {
            let _guard = span.enter();
            compute(input, anchor_hash, &metrics)
        })
    }

    #[cfg(not(feature = "rayon"))]
    let _ = span;

    compute(input, anchor_hash, metrics)
}

#[tracing::instrument(
    level = "trace",
    target = "storage::overlay::manager",
    skip_all,
    fields(
        anchor_hash = %anchor_hash,
        block_count = tracing::field::Empty,
        parent_overlay = tracing::field::Empty,
        elapsed_us = tracing::field::Empty,
    )
)]
pub(crate) fn compute_state_trie_overlay<N: NodePrimitives>(
    input: ComputeOverlayInput<N, TrieInputSorted>,
    anchor_hash: B256,
    metrics: &StateTrieOverlayMetrics,
) -> TrieInputSorted {
    let started_at = Instant::now();
    let block_count = match &input {
        ComputeOverlayInput::ExtendCached { .. } => 1,
        ComputeOverlayInput::MergeBlocks(blocks) => blocks.len(),
    };
    let parent_overlay = matches!(&input, ComputeOverlayInput::ExtendCached { .. });
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay);

    let overlay = match input {
        ComputeOverlayInput::ExtendCached { block, parent_input } => {
            let trie_data = block.trie_data();

            trace!(
                target: "storage::overlay::manager",
                %anchor_hash,
                head = %block.recovered_block().hash(),
                "extending cached parent state trie overlay"
            );

            let mut parent_input = parent_input;
            extend_overlay(
                Arc::make_mut(&mut parent_input),
                &trie_data.hashed_state,
                &trie_data.trie_updates,
            );
            Arc::unwrap_or_clone(parent_input)
        }
        ComputeOverlayInput::MergeBlocks(blocks) => merge_blocks(blocks),
    };

    let elapsed = started_at.elapsed();
    metrics.overlay_computation_duration_seconds.record(elapsed.as_secs_f64());
    tracing::Span::current().record("elapsed_us", elapsed.as_micros() as u64);
    debug!(
        target: "storage::overlay::manager",
        %anchor_hash,
        block_count,
        parent_overlay,
        ?elapsed,
        "computed state trie overlay"
    );

    overlay
}

fn merge_blocks<N: NodePrimitives>(blocks: Vec<ExecutedBlock<N>>) -> TrieInputSorted {
    let trie_data = blocks.iter().map(ExecutedBlock::trie_data).collect::<Vec<_>>();

    #[cfg(feature = "rayon")]
    let (nodes, state) = rayon::join(
        || {
            TrieUpdatesSorted::merge_batch(
                trie_data.iter().map(|data| Arc::clone(&data.trie_updates)),
            )
        },
        || {
            HashedPostStateSorted::merge_batch(
                trie_data.iter().map(|data| Arc::clone(&data.hashed_state)),
            )
        },
    );

    #[cfg(not(feature = "rayon"))]
    let (nodes, state) = (
        TrieUpdatesSorted::merge_batch(trie_data.iter().map(|data| Arc::clone(&data.trie_updates))),
        HashedPostStateSorted::merge_batch(
            trie_data.iter().map(|data| Arc::clone(&data.hashed_state)),
        ),
    );

    TrieInputSorted::new(nodes, state, Default::default())
}

fn extend_overlay(
    overlay: &mut TrieInputSorted,
    hashed_state: &HashedPostStateSorted,
    trie_updates: &TrieUpdatesSorted,
) {
    #[cfg(feature = "rayon")]
    {
        rayon::join(
            || {
                if !hashed_state.is_empty() {
                    Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state);
                }
            },
            || {
                if !trie_updates.is_empty() {
                    Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(trie_updates);
                }
            },
        );
    }

    #[cfg(not(feature = "rayon"))]
    {
        if !hashed_state.is_empty() {
            Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state);
        }
        if !trie_updates.is_empty() {
            Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(trie_updates);
        }
    }
}

pub(crate) fn compute_execution_overlay<N: NodePrimitives>(
    input: ComputeOverlayInput<N, ExecutionOverlay>,
    anchor_hash: B256,
    metrics: &ExecutionOverlayMetrics,
) -> ExecutionOverlay {
    let started_at = Instant::now();
    let block_count = match &input {
        ComputeOverlayInput::ExtendCached { .. } => 1,
        ComputeOverlayInput::MergeBlocks(blocks) => blocks.len(),
    };
    let parent_overlay = matches!(&input, ComputeOverlayInput::ExtendCached { .. });
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay);

    let overlay = match input {
        ComputeOverlayInput::ExtendCached { block, parent_input } => {
            let mut parent_input = parent_input;
            Arc::make_mut(&mut parent_input).extend_block(&block);
            Arc::unwrap_or_clone(parent_input)
        }
        ComputeOverlayInput::MergeBlocks(blocks) => {
            let mut overlay = ExecutionOverlay::default();
            for block in blocks.iter().rev() {
                overlay.extend_block(block);
            }
            overlay
        }
    };

    let elapsed = started_at.elapsed();
    metrics.overlay_computation_duration_seconds.record(elapsed.as_secs_f64());
    tracing::Span::current().record("elapsed_us", elapsed.as_micros() as u64);
    debug!(
        target: "storage::overlay::manager",
        %anchor_hash,
        block_count,
        parent_overlay,
        ?elapsed,
        "computed execution overlay"
    );

    overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::HashMap;
    use reth_trie_common::bundle_state::{AccountStatus, BundleAccount};

    #[reth_concurrency::test]
    fn execution_overlay_extends_bundle_state() {
        let address = Address::with_last_byte(1);
        let no_storage = Address::with_last_byte(2);
        let slot = U256::from(2);
        let value = U256::from(3);
        let code = Bytecode::new_raw(vec![0x60, 0x00].into());
        let code_hash = code.hash_slow();
        let account = AccountInfo::new(U256::from(5), 4, code_hash, Some(code.clone()));
        let state = BundleState::builder(0..=0)
            .state_present_account_info(address, account.clone())
            .state_present_account_info(no_storage, AccountInfo::default())
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, value))]))
            .contract(code_hash, code.clone())
            .build();
        let mut overlay = ExecutionOverlay::default();
        overlay.extend_state(&state);

        let stored_account = overlay.accounts[&address].as_ref().unwrap();
        assert_eq!(stored_account, &account);
        assert_eq!(stored_account.code, Some(code.clone()));
        assert_eq!(overlay.storage[&address][&slot], value);
        assert!(!overlay.storage.contains_key(&no_storage));
        assert_eq!(overlay.storage_value(no_storage, slot), None);
        assert_eq!(overlay.code_hashes[&code_hash], code);
    }

    #[reth_concurrency::test]
    fn execution_overlay_zeroes_unobserved_storage_for_destroyed_accounts() {
        let address = Address::with_last_byte(1);
        let mut state = BundleState::default();
        state.state.insert(
            address,
            BundleAccount::new(
                Some(AccountInfo::default()),
                None,
                Default::default(),
                AccountStatus::Destroyed,
            ),
        );

        let mut overlay = ExecutionOverlay::default();
        overlay.storage.entry(address).or_default().insert(U256::ZERO, U256::from(1));
        overlay.extend_state(&state);

        assert!(!overlay.storage.contains_key(&address));
        assert_eq!(overlay.storage_value(address, U256::ZERO), Some(U256::ZERO));
    }

    #[reth_concurrency::test]
    fn execution_overlay_storage_changes_preserve_account_fallback() {
        let address = Address::with_last_byte(1);
        let info = AccountInfo::default().with_balance(U256::from(5));
        let state = BundleState::builder(0..=0)
            .state_original_account_info(address, info.clone())
            .state_present_account_info(address, info.clone())
            .state_storage(address, HashMap::from_iter([(U256::ZERO, (U256::ZERO, U256::from(1)))]))
            .build();
        let mut overlay = ExecutionOverlay::default();
        overlay.extend_state(&state);
        assert!(!overlay.accounts.contains_key(&address));
        assert_eq!(overlay.storage_value(address, U256::ZERO), Some(U256::from(1)));

        overlay.accounts.insert(address, Some(info.clone()));
        overlay.extend_state(&state);
        assert_eq!(overlay.accounts[&address], Some(info));
    }
}
