//! Reusable multiproof generation over a fixed state snapshot.

use super::{DeferredValueEncoder, LeafValueEncoder, ProofCalculator, StorageProofCalculator};
use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    prefix_set::{PrefixSet, TriePrefixSets},
    trie_cursor::{TrieCursorFactory, TrieStorageCursor},
    DecodedMultiProofV2, MultiProofTargetsV2,
};
use alloy_primitives::{map::B256Map, B256, U256};
use alloy_rlp::Encodable;
use parking_lot::Mutex;
use reth_execution_errors::StateProofError;
use reth_primitives_traits::Account;
use std::{fmt, marker::PhantomPinned, pin::Pin, sync::Arc};

/// Generates account and storage multiproofs for one immutable state snapshot.
///
/// The calculator owns its cursor factories and may be reused for any number of requests. It stays
/// usable after an error, but must be dropped after a panic during a request. The factories must
/// keep describing the same state for the lifetime of the calculator. Cursor factories control
/// whether their cursors are instrumented.
///
/// `'a` is the lifetime of the cursors opened from the owned factories. It is not tied to a
/// caller-supplied borrow; it only needs to be satisfied by `T` and `H`.
pub struct MultiProofCalculator<'a, T: TrieCursorFactory + 'a, H: HashedCursorFactory + 'a> {
    account_calculator: Option<AccountCalculatorOf<'a, T, H>>,
    storage_calculator: Option<Arc<Mutex<StorageCalculatorOf<'a, T, H>>>>,
    prefix_sets: TriePrefixSets,
    // Cursors borrow these pinned factories. `Drop` releases them first.
    factories: Pin<Box<MultiProofCalculatorFactories<T, H>>>,
}

impl<'a, T: TrieCursorFactory + 'a, H: HashedCursorFactory + 'a> MultiProofCalculator<'a, T, H> {
    /// Creates a calculator for the supplied factories and frozen changed-key prefixes.
    ///
    /// Prefixes invalidate cached branch hashes for keys whose leaf values have changed, and apply
    /// to every request served by this calculator.
    pub fn new(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        prefix_sets: TriePrefixSets,
    ) -> Self {
        Self {
            account_calculator: None,
            storage_calculator: None,
            prefix_sets,
            factories: Box::pin(MultiProofCalculatorFactories {
                trie_cursor_factory,
                hashed_cursor_factory,
                _pin: PhantomPinned,
            }),
        }
    }

    /// Generates account and storage proofs for `targets`.
    ///
    /// An explicit storage address with no slot targets requests its root node. Empty targets
    /// return an empty proof.
    pub fn multiproof(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> Result<DecodedMultiProofV2, StateProofError> {
        if targets.is_empty() {
            return Ok(DecodedMultiProofV2::default())
        }
        let MultiProofTargetsV2 { mut account_targets, storage_targets } = targets;
        let mut proof = DecodedMultiProofV2::default();

        // Open the storage calculator, which account proofs also use for storage roots.
        let storage_calculator = match &mut self.storage_calculator {
            Some(storage_calculator) => storage_calculator,
            slot @ None => {
                // SAFETY: Cursors are private to this calculator and released before factories.
                let factories = unsafe { Self::borrow_factories(self.factories.as_ref()) };
                // Storage cursors are retargeted to each requested account before use.
                let trie_cursor = factories.trie_cursor_factory.storage_trie_cursor(B256::ZERO)?;
                let hashed_cursor =
                    factories.hashed_cursor_factory.hashed_storage_cursor(B256::ZERO)?;
                // Prefixes are taken only after the fallible cursor opens, so a failed
                // initialization can be retried.
                let prefix_sets = std::mem::take(&mut self.prefix_sets.storage_prefix_sets);
                slot.insert(Arc::new(Mutex::new(PrefixSetStorageCalculator {
                    calculator: StorageProofCalculator::new_storage(trie_cursor, hashed_cursor),
                    prefix_sets,
                    hashed_address: None,
                })))
            }
        };

        // Calculate account proofs, computing storage roots inline.
        if !account_targets.is_empty() {
            let account_calculator = match &mut self.account_calculator {
                Some(account_calculator) => account_calculator,
                slot @ None => {
                    // SAFETY: As above, cursors are released before factories.
                    let factories = unsafe { Self::borrow_factories(self.factories.as_ref()) };
                    let trie_cursor = factories.trie_cursor_factory.account_trie_cursor()?;
                    let hashed_cursor = factories.hashed_cursor_factory.hashed_account_cursor()?;
                    let prefix_set = std::mem::take(&mut self.prefix_sets.account_prefix_set);
                    slot.insert(
                        ProofCalculator::new(trie_cursor, hashed_cursor)
                            .with_prefix_set(prefix_set),
                    )
                }
            };
            let mut encoder =
                AccountValueEncoder { storage_calculator: storage_calculator.clone() };
            proof.account_proofs = account_calculator.proof(&mut encoder, &mut account_targets)?;
        }

        // Calculate storage proofs, or root nodes for accounts without slot targets.
        if !storage_targets.is_empty() {
            let mut storage_calculator = storage_calculator.lock();
            for (hashed_address, mut targets) in storage_targets {
                let calculator = storage_calculator.calculator_for(hashed_address);
                let nodes = if targets.is_empty() {
                    vec![calculator.storage_root_node(hashed_address)?]
                } else {
                    calculator.storage_proof(hashed_address, &mut targets)?
                };
                proof.storage_proofs.insert(hashed_address, nodes);
            }
        }
        Ok(proof)
    }

    /// Extends the borrow of the pinned factories to the cursor lifetime `'a`.
    ///
    /// # Safety
    ///
    /// Every reference, cursor and encoder derived from the result must be dropped before the
    /// factories, and none may escape this calculator.
    const unsafe fn borrow_factories(
        factories: Pin<&MultiProofCalculatorFactories<T, H>>,
    ) -> &'a MultiProofCalculatorFactories<T, H> {
        // SAFETY: The factories are pinned and immutable, and the caller keeps every borrower
        // inside this calculator, which releases them before the factories.
        unsafe { &*(factories.get_ref() as *const MultiProofCalculatorFactories<T, H>) }
    }
}

impl<'a, T: TrieCursorFactory + 'a, H: HashedCursorFactory + 'a> Drop
    for MultiProofCalculator<'a, T, H>
{
    fn drop(&mut self) {
        // Cursors borrow `factories`, so release them before the fields drop.
        self.account_calculator = None;
        self.storage_calculator = None;
    }
}

impl<T: TrieCursorFactory, H: HashedCursorFactory> fmt::Debug for MultiProofCalculator<'_, T, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiProofCalculator")
            .field("account_initialized", &self.account_calculator.is_some())
            .field("storage_initialized", &self.storage_calculator.is_some())
            .finish_non_exhaustive()
    }
}

/// Account calculator opened from the cursor factories of a [`MultiProofCalculator`].
type AccountCalculatorOf<'a, T, H> = ProofCalculator<
    <T as TrieCursorFactory>::AccountTrieCursor<'a>,
    <H as HashedCursorFactory>::AccountCursor<'a>,
    AccountValueEncoder<
        <T as TrieCursorFactory>::StorageTrieCursor<'a>,
        <H as HashedCursorFactory>::StorageCursor<'a>,
    >,
>;

/// Storage calculator opened from the cursor factories of a [`MultiProofCalculator`].
type StorageCalculatorOf<'a, T, H> = PrefixSetStorageCalculator<
    <T as TrieCursorFactory>::StorageTrieCursor<'a>,
    <H as HashedCursorFactory>::StorageCursor<'a>,
>;

/// Cursor factories owned by a [`MultiProofCalculator`] at a stable address.
struct MultiProofCalculatorFactories<T, H> {
    trie_cursor_factory: T,
    hashed_cursor_factory: H,
    _pin: PhantomPinned,
}

/// Storage proof calculator that applies each account's changed-slot prefixes.
///
/// All storage calculations go through this type so the prefixes always match the account being
/// calculated.
struct PrefixSetStorageCalculator<TC, HC> {
    calculator: StorageProofCalculator<TC, HC>,
    /// Changed-slot prefixes keyed by hashed address.
    prefix_sets: B256Map<PrefixSet>,
    /// Account whose prefixes are currently applied to `calculator`.
    hashed_address: Option<B256>,
}

impl<TC: TrieStorageCursor, HC: HashedStorageCursor<Value = U256>>
    PrefixSetStorageCalculator<TC, HC>
{
    fn calculator_for(&mut self, hashed_address: B256) -> &mut StorageProofCalculator<TC, HC> {
        if self.hashed_address != Some(hashed_address) {
            self.calculator
                .set_prefix_set(self.prefix_sets.get(&hashed_address).cloned().unwrap_or_default());
            self.hashed_address = Some(hashed_address);
        }
        &mut self.calculator
    }
}

/// Account value encoder that computes storage roots with a shared storage calculator.
struct AccountValueEncoder<TC, HC> {
    // Arc<Mutex<_>> keeps the calculator Send whenever its cursors are. The mutex never poisons,
    // which is why the calculator must not be reused after a panic.
    storage_calculator: Arc<Mutex<PrefixSetStorageCalculator<TC, HC>>>,
}

impl<TC: TrieStorageCursor, HC: HashedStorageCursor<Value = U256>> LeafValueEncoder
    for AccountValueEncoder<TC, HC>
{
    type Value = Account;
    type DeferredEncoder = AccountDeferredValueEncoder<TC, HC>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Account,
    ) -> Self::DeferredEncoder {
        AccountDeferredValueEncoder {
            storage_calculator: self.storage_calculator.clone(),
            hashed_address,
            account,
        }
    }
}

/// Deferred account encoder that computes the account's storage root when encoded.
struct AccountDeferredValueEncoder<TC, HC> {
    storage_calculator: Arc<Mutex<PrefixSetStorageCalculator<TC, HC>>>,
    hashed_address: B256,
    account: Account,
}

impl<TC: TrieStorageCursor, HC: HashedStorageCursor<Value = U256>> DeferredValueEncoder
    for AccountDeferredValueEncoder<TC, HC>
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let root = self
            .storage_calculator
            .lock()
            .calculator_for(self.hashed_address)
            .storage_root(self.hashed_address)?;
        self.account.into_trie_account(root).encode(buf);
        Ok(())
    }
}
