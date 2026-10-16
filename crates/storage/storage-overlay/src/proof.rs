//! Reusable V2 proof calculators for a provider's fixed overlay snapshot.

use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use parking_lot::Mutex;
use reth_db_api::{tables, transaction::DbTx};
use reth_errors::ProviderResult;
use reth_execution_errors::StateProofError;
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursor, HashedStorageCursor},
    proof_v2::{DeferredValueEncoder, LeafValueEncoder, ProofCalculator, StorageProofCalculator},
    trie_cursor::{InMemoryTrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::TrieUpdatesSorted,
    DecodedMultiProofV2, HashedPostStateSorted, MultiProofTargetsV2, TrieInputSorted, TrieType,
};
use reth_trie_db::{
    DatabaseAccountTrieCursor, DatabaseHashedAccountCursor, DatabaseHashedCursorFactory,
    DatabaseHashedStorageCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
    LegacyKeyAdapter, PackedKeyAdapter, TrieTableAdapter,
};
use std::{cell::RefCell, sync::Arc};

#[cfg(feature = "metrics")]
mod metrics;

#[cfg(feature = "metrics")]
use self::metrics::{instrument_hashed, instrument_trie, HashedCursor, TrieCursor};

#[cfg(not(feature = "metrics"))]
type TrieCursor<C> = C;
#[cfg(not(feature = "metrics"))]
type HashedCursor<C> = C;

/// A provider-local cache, allocated only when its first proof is requested.
pub(super) type ProofCalculatorCache<TX> = RefCell<Option<Box<ProofCalculators<TX>>>>;

/// The cache chooses the database key encoding once, before opening any cursors.
pub(super) enum ProofCalculators<TX: DbTx> {
    Legacy(Calculators<TX, LegacyKeyAdapter>),
    Packed(Calculators<TX, PackedKeyAdapter>),
}

impl<TX: DbTx> ProofCalculators<TX> {
    pub(super) const fn new(is_v2: bool, overlay: TrieInputSorted) -> Self {
        if is_v2 {
            Self::Packed(Calculators::new(overlay))
        } else {
            Self::Legacy(Calculators::new(overlay))
        }
    }

    pub(super) fn multiproof(
        &mut self,
        tx: &TX,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        match self {
            Self::Legacy(calculators) => calculators.multiproof(tx, targets),
            Self::Packed(calculators) => calculators.multiproof(tx, targets),
        }
    }
}

type AccountTrieCursor<TX, A> = TrieCursor<
    InMemoryTrieCursor<
        'static,
        DatabaseAccountTrieCursor<
            <TX as DbTx>::Cursor<<A as TrieTableAdapter>::AccountTrieTable>,
            A,
        >,
    >,
>;
type StorageTrieCursor<TX, A> = TrieCursor<
    InMemoryTrieCursor<
        'static,
        DatabaseStorageTrieCursor<
            <TX as DbTx>::DupCursor<<A as TrieTableAdapter>::StorageTrieTable>,
            A,
        >,
    >,
>;
type AccountHashedCursor<TX> = HashedCursor<
    HashedPostStateCursor<
        'static,
        DatabaseHashedAccountCursor<<TX as DbTx>::Cursor<tables::HashedAccounts>>,
        Option<Account>,
    >,
>;
type StorageHashedCursor<TX> = HashedCursor<
    HashedPostStateCursor<
        'static,
        DatabaseHashedStorageCursor<<TX as DbTx>::DupCursor<tables::HashedStorages>>,
        U256,
    >,
>;
type StorageCalculator<TX, A> =
    StorageProofCalculator<StorageTrieCursor<TX, A>, StorageHashedCursor<TX>>;
type AccountCalculator<TX, A> = ProofCalculator<
    AccountTrieCursor<TX, A>,
    AccountHashedCursor<TX>,
    AccountValueEncoder<StorageTrieCursor<TX, A>, StorageHashedCursor<TX>>,
>;

/// Account workers initialize both calculators; storage workers initialize only `storage`.
///
/// Cursors borrow the immutable Arc allocations in `overlay` with erased lifetimes. Calculators
/// and their deferred encoders must remain private and be dropped before `overlay`.
pub(super) struct Calculators<TX: DbTx, A: TrieTableAdapter> {
    account: Option<AccountCalculator<TX, A>>,
    storage: Option<Arc<Mutex<StorageCalculator<TX, A>>>>,
    overlay: TrieInputSorted,
}

impl<TX: DbTx, A: TrieTableAdapter> Calculators<TX, A> {
    const fn new(overlay: TrieInputSorted) -> Self {
        Self { account: None, storage: None, overlay }
    }

    fn multiproof(
        &mut self,
        tx: &TX,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        let MultiProofTargetsV2 { mut account_targets, storage_targets } = targets;
        let mut proof = DecodedMultiProofV2::default();
        let storage = match &mut self.storage {
            Some(storage) => storage,
            slot @ None => {
                // SAFETY: The private calculators cannot outlive their immutable backing overlay,
                // which is dropped after both calculators and all their deferred encoders.
                let (nodes, state) = unsafe { borrow_overlay(&self.overlay) };
                let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
                let hashed_factory = DatabaseHashedCursorFactory::new(tx);
                let trie_cursor = InMemoryTrieCursor::new_storage(
                    trie_factory.storage_trie_cursor(B256::ZERO)?,
                    nodes,
                    B256::ZERO,
                );
                let hashed_cursor = HashedPostStateCursor::new_storage(
                    hashed_factory.hashed_storage_cursor(B256::ZERO)?,
                    state,
                    B256::ZERO,
                );
                slot.insert(Arc::new(Mutex::new(StorageProofCalculator::new_storage(
                    instrument_trie(trie_cursor, TrieType::Storage),
                    instrument_hashed(hashed_cursor, TrieType::Storage),
                ))))
            }
        };

        if !account_targets.is_empty() {
            let calculator = match &mut self.account {
                Some(calculator) => calculator,
                slot @ None => {
                    // SAFETY: As above, the overlay outlives both private calculators and encoders.
                    let (nodes, state) = unsafe { borrow_overlay(&self.overlay) };
                    let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
                    let hashed_factory = DatabaseHashedCursorFactory::new(tx);
                    let trie_cursor =
                        InMemoryTrieCursor::new_account(trie_factory.account_trie_cursor()?, nodes);
                    let hashed_cursor = HashedPostStateCursor::new_account(
                        hashed_factory.hashed_account_cursor()?,
                        state,
                    );
                    slot.insert(ProofCalculator::new(
                        instrument_trie(trie_cursor, TrieType::State),
                        instrument_hashed(hashed_cursor, TrieType::State),
                    ))
                }
            };
            let mut encoder = AccountValueEncoder { storage_calculator: storage.clone() };
            proof.account_proofs = calculator.proof(&mut encoder, &mut account_targets)?;
        }

        if !storage_targets.is_empty() {
            let mut calculator = storage.lock();
            for (address, mut targets) in storage_targets {
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

struct AccountValueEncoder<TC, HC> {
    // Arc<Mutex<_>> is needed only to keep the provider Send;
    // otherwise Rc<RefCell<_>> would suffice.
    storage_calculator: Arc<Mutex<StorageProofCalculator<TC, HC>>>,
}

impl<TC, HC> LeafValueEncoder for AccountValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = U256>,
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

struct AccountDeferredValueEncoder<TC, HC> {
    storage_calculator: Arc<Mutex<StorageProofCalculator<TC, HC>>>,
    hashed_address: B256,
    account: Account,
}

impl<TC, HC> DeferredValueEncoder for AccountDeferredValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = U256>,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let root = self.storage_calculator.lock().storage_root(self.hashed_address)?;
        self.account.into_trie_account(root).encode(buf);
        Ok(())
    }
}

#[cfg(not(feature = "metrics"))]
fn instrument_trie<C>(cursor: C, _trie_type: TrieType) -> C {
    cursor
}

#[cfg(not(feature = "metrics"))]
fn instrument_hashed<C>(cursor: C, _trie_type: TrieType) -> C {
    cursor
}

/// Borrows the overlay's stable Arc allocations with erased lifetimes.
///
/// # Safety
///
/// The caller must retain the backing Arcs, without mutating their contents, until all returned
/// references and cursors borrowing them have been dropped. These references must not escape
/// the owner that enforces this lifetime and drop order.
unsafe fn borrow_overlay(
    overlay: &TrieInputSorted,
) -> (&'static TrieUpdatesSorted, &'static HashedPostStateSorted) {
    // SAFETY: Arc allocations remain at stable addresses while retained by the caller, which
    // guarantees their immutability and that all borrowers are dropped before the backing Arcs.
    unsafe { (&*Arc::as_ptr(&overlay.nodes), &*Arc::as_ptr(&overlay.state)) }
}
