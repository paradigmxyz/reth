//! Multiproof generators over database trie tables with an immutable in-memory overlay.

use reth_db_api::transaction::DbTx;
use reth_errors::ProviderResult;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::TriePrefixSets,
    proof_v2::{MultiProofGenerator, ProofCursorMetrics},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdatesSorted,
    DecodedMultiProofV2, HashedPostStateSorted, MultiProofTargetsV2, TrieInputSorted,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::sync::Arc;

/// Multiproof generator for the database's trie table layout.
///
/// The generator borrows a transaction and overlay it does not own, so its owner must keep both
/// alive and at stable addresses for as long as the generator exists.
pub(crate) enum OverlayMultiProofGenerator<TX: DbTx + 'static> {
    /// Generator over legacy trie tables.
    Legacy(TableMultiProofGenerator<TX, LegacyKeyAdapter>),
    /// Generator over packed trie tables.
    Packed(TableMultiProofGenerator<TX, PackedKeyAdapter>),
}

impl<TX: DbTx + 'static> OverlayMultiProofGenerator<TX> {
    /// Creates a generator over `tx` with `input` layered on top.
    ///
    /// `is_v2` selects the packed trie tables used by v2 storage settings.
    ///
    /// # Safety
    ///
    /// The transaction and the `Arc` allocations in `input` must stay alive and at stable addresses
    /// until the generator drops. Unless the transaction is `Sync`, the generator must not be sent
    /// to another thread separately from the transaction, and the transaction must not be used from
    /// another thread while the generator exists.
    pub(crate) unsafe fn new(
        tx: &TX,
        input: &TrieInputSorted,
        is_v2: bool,
        metrics: ProofCursorMetrics,
    ) -> Self {
        // SAFETY: The caller keeps these allocations alive and in place until the generator drops.
        let (tx, nodes, state) = unsafe {
            (&*(tx as *const TX), &*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state))
        };
        let prefix_sets = input.prefix_sets.clone().freeze();
        if is_v2 {
            Self::Packed(table_multiproof_generator(tx, nodes, state, prefix_sets, metrics))
        } else {
            Self::Legacy(table_multiproof_generator(tx, nodes, state, prefix_sets, metrics))
        }
    }

    /// Generates account and storage proofs for `targets`.
    pub(crate) fn multiproof(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        Ok(match self {
            Self::Legacy(generator) => generator.multiproof(targets)?,
            Self::Packed(generator) => generator.multiproof(targets)?,
        })
    }
}

// SAFETY: `new` requires the generator to move between threads only together with its
// transaction, or the transaction to be `Sync`, so the borrowed transaction is never shared across
// threads without synchronization.
unsafe impl<TX: DbTx + 'static> Send for OverlayMultiProofGenerator<TX> {}

/// Multiproof generator over trie table layout `A`, whose borrows of the transaction and overlay
/// are extended to `'static` by [`OverlayMultiProofGenerator::new`].
type TableMultiProofGenerator<TX, A> = MultiProofGenerator<
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

/// Creates a generator for trie table layout `A`.
fn table_multiproof_generator<TX: DbTx + 'static, A: TrieTableAdapter>(
    tx: &'static TX,
    nodes: &'static TrieUpdatesSorted,
    state: &'static HashedPostStateSorted,
    prefix_sets: TriePrefixSets,
    metrics: ProofCursorMetrics,
) -> TableMultiProofGenerator<TX, A> {
    MultiProofGenerator::new(
        InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), nodes),
        HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), state),
        prefix_sets,
        metrics,
    )
}
