//! Concrete V2 proof generators for legacy and packed database trie tables.

use reth_db_api::transaction::DbTx;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof_v2::ProofV2,
    trie_cursor::InMemoryTrieCursorFactory, updates::TrieUpdatesSorted, HashedPostStateSorted,
    TrieInputSorted,
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

/// Creates a concrete generator borrowing its provider's transaction and overlays.
///
/// # Safety
///
/// The caller must retain the pinned transaction and immutable overlay allocations until the
/// proof drops.
pub(crate) unsafe fn cached_proof_v2<TX: DbTx + 'static, A: TrieTableAdapter>(
    tx: &TX,
    input: &TrieInputSorted,
) -> ProofWithOverlay<TX, A> {
    // SAFETY: The caller guarantees these allocations remain stable and outlive the generator.
    let (tx, nodes, state) =
        unsafe { (&*(tx as *const TX), &*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state)) };
    ProofV2::new(
        InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), nodes),
        HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), state),
        input.prefix_sets.clone().freeze(),
    )
}
