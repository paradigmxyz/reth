//! Reusable V2 account and storage proof generation for a fixed state snapshot.

use self::value::AccountValueEncoder;
use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    prefix_set::{PrefixSet, TriePrefixSets},
    trie_cursor::{TrieCursorFactory, TrieStorageCursor},
    DecodedMultiProofV2, MultiProofTargetsV2, TrieType,
};
use alloy_primitives::{map::B256Map, B256, U256};
use parking_lot::Mutex;
use reth_execution_errors::StateProofError;
use std::{fmt, marker::PhantomPinned, pin::Pin, sync::Arc};

mod calculator;
pub use calculator::*;

mod value;
pub use value::*;

mod node;

mod target;
pub(crate) use target::*;

#[cfg(feature = "metrics")]
mod metrics;

#[cfg(feature = "metrics")]
use self::metrics::{instrument_hashed, instrument_trie, HashedCursor, TrieCursor};

#[cfg(not(feature = "metrics"))]
type TrieCursor<C> = C;
#[cfg(not(feature = "metrics"))]
type HashedCursor<C> = C;

/// Reuses proof calculators and open cursors for one immutable factory snapshot.
///
/// Account and storage calculators initialize independently on first use. Failed initialization
/// can be retried, and calculations reset their traversal state after both success and errors.
/// Factories must continue to describe the same state for the lifetime of this generator.
pub struct ProofV2<'a, T: TrieCursorFactory + 'a, H: HashedCursorFactory + 'a> {
    account: Option<AccountCalculator<'a, T, H>>,
    storage: Option<Arc<Mutex<StorageCalculator<'a, T, H>>>>,
    prefix_sets: TriePrefixSets,
    // Cursors borrow these pinned factories and must be dropped before them.
    factories: Pin<Box<Factories<T, H>>>,
}

impl<'a, T: TrieCursorFactory + 'a, H: HashedCursorFactory + 'a> ProofV2<'a, T, H> {
    /// Creates a proof generator for the supplied factories and frozen changed-key prefixes.
    ///
    /// Prefixes invalidate cached branch hashes for keys whose leaf values have changed.
    pub fn new(trie: T, hashed: H, prefix_sets: TriePrefixSets) -> Self {
        Self {
            account: None,
            storage: None,
            prefix_sets,
            factories: Box::pin(Factories { trie, hashed, _pin: PhantomPinned }),
        }
    }

    /// Generates account and storage proofs, retaining calculators for subsequent requests.
    ///
    /// An explicit storage address with no slot targets requests its root node. Empty targets
    /// return an empty proof without opening cursors. Errors leave the generator reusable.
    pub fn multiproof(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> Result<DecodedMultiProofV2, StateProofError> {
        if targets.is_empty() {
            return Ok(DecodedMultiProofV2::default())
        }
        let MultiProofTargetsV2 { mut account_targets, storage_targets } = targets;
        let mut proof = DecodedMultiProofV2::default();
        let storage = match &mut self.storage {
            Some(storage) => storage,
            slot @ None => {
                // SAFETY: Factories are pinned, immutable, private, and dropped after all cursors
                // and deferred encoders. No cursor or extended reference escapes this generator.
                let factories = unsafe { borrow_factories(self.factories.as_ref()) };
                let trie = factories.trie.storage_trie_cursor(B256::ZERO)?;
                let hashed = factories.hashed.hashed_storage_cursor(B256::ZERO)?;
                let prefix_sets = std::mem::take(&mut self.prefix_sets.storage_prefix_sets);
                slot.insert(Arc::new(Mutex::new(CachedStorage {
                    calculator: StorageProofCalculator::new_storage(
                        instrument_trie(trie, TrieType::Storage),
                        instrument_hashed(hashed, TrieType::Storage),
                    ),
                    prefix_sets,
                    address: None,
                })))
            }
        };
        if !account_targets.is_empty() {
            let calculator = match &mut self.account {
                Some(calculator) => calculator,
                slot @ None => {
                    // SAFETY: As above, all borrowers are private and dropped before factories.
                    let factories = unsafe { borrow_factories(self.factories.as_ref()) };
                    let trie = factories.trie.account_trie_cursor()?;
                    let hashed = factories.hashed.hashed_account_cursor()?;
                    slot.insert(
                        ProofCalculator::new(
                            instrument_trie(trie, TrieType::State),
                            instrument_hashed(hashed, TrieType::State),
                        )
                        .with_prefix_set(std::mem::take(&mut self.prefix_sets.account_prefix_set)),
                    )
                }
            };
            let mut encoder = AccountValueEncoder { storage_calculator: storage.clone() };
            proof.account_proofs = calculator.proof(&mut encoder, &mut account_targets)?;
        }
        if !storage_targets.is_empty() {
            let mut storage = storage.lock();
            for (address, mut targets) in storage_targets {
                let calculator = storage.calculator(address);
                let nodes = if targets.is_empty() {
                    vec![calculator.storage_root_node(address)?]
                } else {
                    calculator.storage_proof(address, &mut targets)?
                };
                proof.storage_proofs.insert(address, nodes);
            }
        }
        Ok(proof)
    }
}

impl<T: TrieCursorFactory, H: HashedCursorFactory> fmt::Debug for ProofV2<'_, T, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofV2")
            .field("account_initialized", &self.account.is_some())
            .field("storage_initialized", &self.storage.is_some())
            .finish_non_exhaustive()
    }
}

type StorageCalculator<'a, T, H> = CachedStorage<
    TrieCursor<<T as TrieCursorFactory>::StorageTrieCursor<'a>>,
    HashedCursor<<H as HashedCursorFactory>::StorageCursor<'a>>,
>;
type AccountCalculator<'a, T, H> = ProofCalculator<
    TrieCursor<<T as TrieCursorFactory>::AccountTrieCursor<'a>>,
    HashedCursor<<H as HashedCursorFactory>::AccountCursor<'a>>,
    AccountValueEncoder<
        TrieCursor<<T as TrieCursorFactory>::StorageTrieCursor<'a>>,
        HashedCursor<<H as HashedCursorFactory>::StorageCursor<'a>>,
    >,
>;

struct Factories<T, H> {
    trie: T,
    hashed: H,
    _pin: PhantomPinned,
}

/// Extends the borrow of pinned factories for private cached cursors.
///
/// # Safety
///
/// The factories must remain pinned and immutable until all references, cursors and encoders
/// borrowing them have been dropped. No borrower may escape the owner enforcing that drop order.
const unsafe fn borrow_factories<'a, T, H>(
    factories: Pin<&Factories<T, H>>,
) -> &'a Factories<T, H> {
    // SAFETY: The caller retains the pinned allocation until every borrower is dropped.
    unsafe { &*(factories.get_ref() as *const Factories<T, H>) }
}

struct CachedStorage<TC, HC> {
    calculator: StorageProofCalculator<TC, HC>,
    prefix_sets: B256Map<PrefixSet>,
    address: Option<B256>,
}

impl<TC: TrieStorageCursor, HC: HashedStorageCursor<Value = U256>> CachedStorage<TC, HC> {
    fn calculator(&mut self, address: B256) -> &mut StorageProofCalculator<TC, HC> {
        if self.address != Some(address) {
            self.calculator
                .set_prefix_set(self.prefix_sets.get(&address).cloned().unwrap_or_default());
            self.address = Some(address);
        }
        &mut self.calculator
    }
}

#[cfg(not(feature = "metrics"))]
const fn instrument_trie<C>(cursor: C, _trie_type: TrieType) -> C {
    cursor
}

#[cfg(not(feature = "metrics"))]
const fn instrument_hashed<C>(cursor: C, _trie_type: TrieType) -> C {
    cursor
}

/// Target to use with the `tracing` crate.
static TRACE_TARGET: &str = "trie::proof_v2";
