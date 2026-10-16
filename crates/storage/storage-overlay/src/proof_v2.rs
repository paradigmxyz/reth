//! Concrete V2 proof generators for legacy and packed database trie tables.

use reth_db_api::transaction::DbTx;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::TriePrefixSets,
    proof_v2::{ProofV2, ProofV2Metrics},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdatesSorted,
    HashedPostStateSorted, TrieInputSorted,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::sync::Arc;

/// Selects the concrete proof generator for the database's trie table format.
pub(crate) enum DatabaseProofV2<TX: DbTx + 'static> {
    Legacy(ProofWithOverlay<TX, LegacyKeyAdapter>),
    Packed(ProofWithOverlay<TX, PackedKeyAdapter>),
}

impl<TX: DbTx + 'static> DatabaseProofV2<TX> {
    /// Creates a generator for the selected trie table format.
    ///
    /// # Safety
    ///
    /// The transaction and immutable overlay allocations must remain at stable addresses and
    /// outlive the generator. The generator must only move with its owning non-Sync provider,
    /// and the transaction must not be accessed concurrently elsewhere.
    pub(crate) unsafe fn new(
        tx: &TX,
        input: &TrieInputSorted,
        is_v2: bool,
        metrics: ProofV2Metrics,
    ) -> Self {
        // SAFETY: The caller retains these stable allocations until the generator drops.
        let (tx, nodes, state) = unsafe {
            (&*(tx as *const TX), &*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state))
        };
        let prefix_sets = input.prefix_sets.clone().freeze();
        if is_v2 {
            Self::Packed(proof_with_overlay(tx, nodes, state, prefix_sets, metrics))
        } else {
            Self::Legacy(proof_with_overlay(tx, nodes, state, prefix_sets, metrics))
        }
    }
}

// SAFETY: The unsafe constructor requires this generator to move only with its owning non-Sync
// provider, which retains its Send transaction and immutable overlays without concurrent access.
unsafe impl<TX: DbTx + 'static> Send for DatabaseProofV2<TX> {}

type ProofWithOverlay<TX, A> = ProofV2<
    'static,
    InMemoryTrieCursorFactory<
        DatabaseTrieCursorFactory<&'static TX, A>,
        &'static TrieUpdatesSorted,
    >,
    HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<&'static TX>,
        &'static HashedPostStateSorted,
    >,
>;

fn proof_with_overlay<TX: DbTx + 'static, A: TrieTableAdapter>(
    tx: &'static TX,
    nodes: &'static TrieUpdatesSorted,
    state: &'static HashedPostStateSorted,
    prefix_sets: TriePrefixSets,
    metrics: ProofV2Metrics,
) -> ProofWithOverlay<TX, A> {
    ProofV2::new(
        InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), nodes),
        HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), state),
        prefix_sets,
        metrics,
    )
}
