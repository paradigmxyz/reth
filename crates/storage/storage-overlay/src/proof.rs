//! Provider-owned V2 proof snapshots and their erased transaction lifetimes.

use reth_db_api::transaction::DbTx;
use reth_errors::ProviderResult;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof_v2::ProofV2,
    trie_cursor::InMemoryTrieCursorFactory, updates::TrieUpdatesSorted, DecodedMultiProofV2,
    HashedPostStateSorted, MultiProofTargetsV2, TrieInputSorted,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::{cell::RefCell, sync::Arc};

pub(super) type ProofCache = RefCell<Option<CachedProof>>;

/// A cache entry belongs exclusively to its provider and is dropped before that provider.
pub(super) struct CachedProof {
    pub(super) requires_masks: bool,
    proof: Box<dyn Multiproof>,
}

// SAFETY: Construction is restricted to a Send transaction owned by the enclosing provider.
// That provider is not Sync and moves with this cache, so transaction references and cursors
// are only accessed on its owning thread. The cache never escapes the provider.
unsafe impl Send for CachedProof {}

impl CachedProof {
    /// Creates a generator borrowing the enclosing provider's stable transaction allocation.
    ///
    /// # Safety
    ///
    /// The transaction must remain at this address and outlive the cache. The cache must only
    /// move with its provider, and the transaction must not be accessed concurrently elsewhere.
    pub(super) unsafe fn new<TX: DbTx>(
        tx: &TX,
        input: TrieInputSorted,
        is_v2: bool,
        requires_masks: bool,
    ) -> Self {
        let proof: Box<dyn Multiproof + '_> = if is_v2 {
            Box::new(OverlayProof::<_, PackedKeyAdapter>::new(tx, input))
        } else {
            Box::new(OverlayProof::<_, LegacyKeyAdapter>::new(tx, input))
        };
        // SAFETY: The caller retains the stable transaction until this private cache is dropped.
        let proof =
            unsafe { std::mem::transmute::<Box<dyn Multiproof + '_>, Box<dyn Multiproof>>(proof) };
        Self { requires_masks, proof }
    }

    pub(super) fn multiproof(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        self.proof.multiproof(targets)
    }
}

trait Multiproof {
    fn multiproof(&mut self, targets: MultiProofTargetsV2) -> ProviderResult<DecodedMultiProofV2>;
}

struct OverlayProof<'a, TX: DbTx, A: TrieTableAdapter> {
    proof: DbProofV2<'a, TX, A>,
    // All cursors and their factories must be dropped before these backing Arc allocations.
    _input: TrieInputSorted,
}

impl<'a, TX: DbTx, A: TrieTableAdapter> OverlayProof<'a, TX, A> {
    fn new(tx: &'a TX, mut input: TrieInputSorted) -> Self {
        // SAFETY: These immutable Arc allocations stay alive in _input until after proof and all
        // its cursors are dropped. Neither cursors nor the extended references escape this owner.
        let (nodes, state) = unsafe { (&*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state)) };
        Self {
            proof: ProofV2::new(
                InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), nodes),
                HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), state),
                std::mem::take(&mut input.prefix_sets).freeze(),
            ),
            _input: input,
        }
    }
}

impl<TX: DbTx, A: TrieTableAdapter> Multiproof for OverlayProof<'_, TX, A> {
    fn multiproof(&mut self, targets: MultiProofTargetsV2) -> ProviderResult<DecodedMultiProofV2> {
        Ok(self.proof.multiproof(targets)?)
    }
}

type DbProofV2<'a, TX, A> = ProofV2<
    'a,
    InMemoryTrieCursorFactory<DatabaseTrieCursorFactory<&'a TX, A>, &'a TrieUpdatesSorted>,
    HashedPostStateCursorFactory<DatabaseHashedCursorFactory<&'a TX>, &'a HashedPostStateSorted>,
>;
