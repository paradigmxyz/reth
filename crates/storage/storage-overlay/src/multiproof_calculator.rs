//! Multiproof calculators over database trie tables with an immutable in-memory overlay.

use reth_db_api::transaction::DbTx;
use reth_errors::ProviderResult;
use reth_trie::{
    hashed_cursor::{HashedPostStateCursorFactory, InstrumentedHashedCursorFactory},
    prefix_set::TriePrefixSets,
    proof_v2::{MultiProofCalculator, ProofCursorMetrics},
    trie_cursor::{InMemoryTrieCursorFactory, InstrumentedTrieCursorFactory},
    updates::TrieUpdatesSorted,
    DecodedMultiProofV2, HashedPostStateSorted, MultiProofTargetsV2, TrieInputSorted,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::sync::Arc;

/// Multiproof calculator for the database's trie table layout.
///
/// The calculator borrows a transaction and overlay it does not own, so its owner must keep both
/// alive and at stable addresses for as long as the calculator exists.
pub(crate) enum OverlayMultiProofCalculator<TX: DbTx + 'static> {
    /// Calculator over legacy trie tables.
    Legacy(TableMultiProofCalculator<TX, LegacyKeyAdapter>),
    /// Calculator over packed trie tables.
    Packed(TableMultiProofCalculator<TX, PackedKeyAdapter>),
}

impl<TX: DbTx + 'static> OverlayMultiProofCalculator<TX> {
    /// Creates a calculator over `tx` with `input` layered on top.
    ///
    /// `is_v2` selects the packed trie tables used by v2 storage settings.
    ///
    /// # Safety
    ///
    /// The transaction and the `Arc` allocations in `input` must stay alive and at stable addresses
    /// until the calculator drops. Unless the transaction is `Sync`, the calculator must not be
    /// sent to another thread separately from the transaction, and the transaction must not be
    /// used from another thread while the calculator exists.
    pub(crate) unsafe fn new(
        tx: &TX,
        input: &TrieInputSorted,
        is_v2: bool,
        metrics: ProofCursorMetrics,
    ) -> Self {
        // SAFETY: The caller keeps these allocations alive and in place until the calculator drops.
        let (tx, nodes, state) = unsafe {
            (&*(tx as *const TX), &*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state))
        };
        let prefix_sets = input.prefix_sets.clone().freeze();
        if is_v2 {
            Self::Packed(table_multiproof_calculator(tx, nodes, state, prefix_sets, metrics))
        } else {
            Self::Legacy(table_multiproof_calculator(tx, nodes, state, prefix_sets, metrics))
        }
    }

    /// Generates account and storage proofs for `targets`.
    pub(crate) fn multiproof(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        Ok(match self {
            Self::Legacy(calculator) => calculator.multiproof(targets)?,
            Self::Packed(calculator) => calculator.multiproof(targets)?,
        })
    }
}

// SAFETY: `new` requires the calculator to move between threads only together with its
// transaction, or the transaction to be `Sync`, so the borrowed transaction is never shared across
// threads without synchronization.
unsafe impl<TX: DbTx + 'static> Send for OverlayMultiProofCalculator<TX> {}

/// Multiproof calculator over trie table layout `A`, whose borrows of the transaction and overlay
/// are extended to `'static` by [`OverlayMultiProofCalculator::new`].
type TableMultiProofCalculator<TX, A> = MultiProofCalculator<
    'static,
    InstrumentedTrieCursorFactory<
        InMemoryTrieCursorFactory<
            DatabaseTrieCursorFactory<&'static TX, A>,
            &'static TrieUpdatesSorted,
        >,
    >,
    InstrumentedHashedCursorFactory<
        HashedPostStateCursorFactory<
            DatabaseHashedCursorFactory<&'static TX>,
            &'static HashedPostStateSorted,
        >,
    >,
>;

/// Creates a calculator for trie table layout `A`.
fn table_multiproof_calculator<TX: DbTx + 'static, A: TrieTableAdapter>(
    tx: &'static TX,
    nodes: &'static TrieUpdatesSorted,
    state: &'static HashedPostStateSorted,
    prefix_sets: TriePrefixSets,
    metrics: ProofCursorMetrics,
) -> TableMultiProofCalculator<TX, A> {
    MultiProofCalculator::new(
        metrics.instrument_trie_factory(InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(tx),
            nodes,
        )),
        metrics.instrument_hashed_factory(HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(tx),
            state,
        )),
        prefix_sets,
    )
}
