//! Provider-owned V2 proof snapshots and their erased transaction lifetimes.

use reth_db_api::transaction::DbTx;
use reth_errors::ProviderResult;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof_v2::ProofV2,
    trie_cursor::InMemoryTrieCursorFactory, DecodedMultiProofV2, MultiProofTargetsV2,
    TrieInputSorted,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::sync::Arc;

type MultiproofFn<'a> =
    Box<dyn FnMut(MultiProofTargetsV2) -> ProviderResult<DecodedMultiProofV2> + 'a>;

/// A proof generator and mask mode owned exclusively by its provider.
pub(super) struct ProviderProofV2 {
    pub(super) requires_masks: bool,
    proof: MultiproofFn<'static>,
}

// SAFETY: Construction is restricted to a Send transaction owned by the enclosing provider.
// That provider is not Sync and moves with this cache, so transaction references and cursors
// are only accessed on its owning thread. The generator never escapes the provider.
unsafe impl Send for ProviderProofV2 {}

impl ProviderProofV2 {
    /// Creates a generator borrowing the enclosing provider's stable transaction allocation.
    ///
    /// # Safety
    ///
    /// The transaction must remain at this address and outlive the generator. The generator must
    /// only move with its provider, and the transaction must not be accessed concurrently
    /// elsewhere.
    pub(super) unsafe fn new<TX: DbTx>(
        tx: &TX,
        input: TrieInputSorted,
        is_v2: bool,
        requires_masks: bool,
    ) -> Self {
        let proof = if is_v2 {
            multiproof_fn::<_, PackedKeyAdapter>(tx, input)
        } else {
            multiproof_fn::<_, LegacyKeyAdapter>(tx, input)
        };
        // SAFETY: The caller retains the stable transaction until this private generator is
        // dropped.
        let proof =
            unsafe { std::mem::transmute::<MultiproofFn<'_>, MultiproofFn<'static>>(proof) };
        Self { requires_masks, proof }
    }

    pub(super) fn multiproof(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        (self.proof)(targets)
    }
}

fn multiproof_fn<TX: DbTx, A: TrieTableAdapter>(
    tx: &TX,
    mut input: TrieInputSorted,
) -> MultiproofFn<'_> {
    // SAFETY: The closure retains these immutable Arc allocations until its generator and all
    // borrowing cursors are dropped. Neither cursors nor extended references escape the closure.
    let (nodes, state) = unsafe { (&*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state)) };
    let proof = ProofV2::new(
        InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::<_, A>::new(tx), nodes),
        HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), state),
        std::mem::take(&mut input.prefix_sets).freeze(),
    );
    let mut proof_and_input = (proof, input);
    Box::new(move |targets| {
        // Borrow the whole tuple so the closure captures both elements. Tuple drop order releases
        // the generator and all its cursors before their backing overlay.
        let snapshot = &mut proof_and_input;
        Ok(snapshot.0.multiproof(targets)?)
    })
}
