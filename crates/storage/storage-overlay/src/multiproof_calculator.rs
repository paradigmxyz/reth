//! Multiproof calculators over database trie tables with an immutable in-memory overlay.

use alloy_primitives::{map::B256Map, B256};
use reth_db_api::{table::Encode, tables, transaction::DbTx};
use reth_errors::ProviderResult;
use reth_trie::{
    hashed_cursor::{HashedPostStateCursorFactory, InstrumentedHashedCursorFactory},
    prefix_set::TriePrefixSets,
    proof_v2::{MultiProofCalculator, ProofCursorMetrics},
    trie_cursor::{InMemoryTrieCursorFactory, InstrumentedTrieCursorFactory},
    updates::TrieUpdatesSorted,
    DecodedMultiProofV2, HashedPostStateSorted, MultiProofTargetsV2, ProofV2Target,
    TrieInputSorted,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::sync::Arc;

/// Number of account trie path lengths looked up for each account proof target.
///
/// Tries of up to about 16^8 accounts rarely have branch nodes deeper than this.
const WARM_ACCOUNT_TRIE_DEPTH: usize = 8;

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
        known_storage_roots: Arc<B256Map<B256>>,
    ) -> Self {
        // SAFETY: The caller keeps these allocations alive and in place until the calculator drops.
        let (tx, nodes, state) = unsafe {
            (&*(tx as *const TX), &*Arc::as_ptr(&input.nodes), &*Arc::as_ptr(&input.state))
        };
        let prefix_sets = input.prefix_sets.clone().freeze();
        if is_v2 {
            Self::Packed(table_multiproof_calculator(
                tx,
                nodes,
                state,
                prefix_sets,
                metrics,
                known_storage_roots,
            ))
        } else {
            Self::Legacy(table_multiproof_calculator(
                tx,
                nodes,
                state,
                prefix_sets,
                metrics,
                known_storage_roots,
            ))
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
    known_storage_roots: Arc<B256Map<B256>>,
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
        known_storage_roots,
    )
}

/// Looks up the hashed accounts and account trie nodes that proofs of `targets` read from `tx`,
/// all at once.
///
/// A proof reads these entries one after another, so each cache miss or page read waits for the
/// previous one. Batched lookups overlap them, leaving the pages cached for the proof.
/// `is_v2` selects the packed trie tables used by v2 storage settings.
pub(crate) fn warm_account_proofs<TX: DbTx>(
    tx: &TX,
    targets: &[ProofV2Target],
    is_v2: bool,
) -> ProviderResult<()> {
    if targets.is_empty() {
        return Ok(())
    }
    let keys: Vec<_> = targets.iter().map(|target| target.key().encode()).collect();
    tx.get_many::<tables::HashedAccounts>(&keys, |_, _| {})?;
    if is_v2 {
        warm_account_trie::<TX, PackedKeyAdapter>(tx, targets)
    } else {
        warm_account_trie::<TX, LegacyKeyAdapter>(tx, targets)
    }
}

/// Looks up the account trie nodes on the paths to `targets` in table layout `A`.
fn warm_account_trie<TX: DbTx, A: TrieTableAdapter>(
    tx: &TX,
    targets: &[ProofV2Target],
) -> ProviderResult<()> {
    let keys: Vec<_> = targets
        .iter()
        .flat_map(|target| {
            (1..=WARM_ACCOUNT_TRIE_DEPTH)
                .map(|len| A::AccountKey::from(target.key_nibbles.slice(..len)).encode())
        })
        .collect();
    tx.get_many::<A::AccountTrieTable>(&keys, |_, _| {})?;
    Ok(())
}
