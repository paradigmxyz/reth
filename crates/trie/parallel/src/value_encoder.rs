use crate::proof_task::{StorageProofResult, StorageProofResultMessage};
use alloy_primitives::{map::B256Map, B256};
use alloy_rlp::Encodable;
use core::cell::RefCell;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use dashmap::DashMap;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    proof_v2::{self, DeferredValueEncoder, LeafValueEncoder},
    trie_cursor::TrieCursorFactory,
    ProofTrieNode,
};
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::debug_span;

/// Returned from [`AsyncAccountValueEncoder`], used to track an async storage root calculation.
pub(crate) enum AsyncAccountDeferredValueEncoder<T, H> {
    /// A storage proof job was dispatched to the worker pool.
    Dispatched {
        hashed_address: B256,
        account: Account,
        proof_result_rx: Result<CrossbeamReceiver<StorageProofResultMessage>, DatabaseError>,
        /// None if results shouldn't be retained for this dispatched proof.
        storage_proof_results: Option<Rc<RefCell<B256Map<Vec<ProofTrieNode>>>>>,
        /// Shared accumulator for storage wait time.
        storage_wait_time: Rc<RefCell<Duration>>,
    },
    /// The storage root was found in cache.
    FromCache { account: Account, root: B256 },
    /// Synchronous storage root computation.
    Sync {
        trie_cursor_factory: Rc<T>,
        hashed_cursor_factory: Rc<H>,
        hashed_address: B256,
        account: Account,
        /// Cache to store computed storage roots for future reuse.
        cached_storage_roots: Arc<DashMap<B256, B256>>,
    },
}

impl<T, H> DeferredValueEncoder for AsyncAccountDeferredValueEncoder<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let (account, root) = match self {
            Self::Dispatched {
                hashed_address,
                account,
                proof_result_rx,
                storage_proof_results,
                storage_wait_time,
            } => {
                let _guard = debug_span!(
                    target: "trie::proof_task",
                    "Waiting for storage proof",
                    ?hashed_address,
                )
                .entered();
                let wait_start = Instant::now();
                let result = proof_result_rx?
                    .recv()
                    .map_err(|_| {
                        StateProofError::Database(DatabaseError::Other(format!(
                            "Storage proof channel closed for {hashed_address:?}",
                        )))
                    })?
                    .result?;
                *storage_wait_time.borrow_mut() += wait_start.elapsed();
                drop(_guard);

                let StorageProofResult::V2 { root: Some(root), proof } = result else {
                    panic!("StorageProofResult is not V2 with root: {result:?}")
                };

                if let Some(storage_proof_results) = storage_proof_results.as_ref() {
                    storage_proof_results.borrow_mut().insert(hashed_address, proof);
                }

                (account, root)
            }
            Self::FromCache { account, root } => (account, root),
            Self::Sync {
                trie_cursor_factory,
                hashed_cursor_factory,
                hashed_address,
                account,
                cached_storage_roots,
            } => {
                let _guard = debug_span!(
                    target: "trie::proof_task",
                    "Sync storage proof",
                    ?hashed_address,
                )
                .entered();
                let trie_cursor = trie_cursor_factory.storage_trie_cursor(hashed_address)?;
                let hashed_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

                let mut storage_proof_calculator =
                    proof_v2::ProofCalculator::new_storage(trie_cursor, hashed_cursor);

                let proof = storage_proof_calculator
                    .storage_proof(hashed_address, &mut [B256::ZERO.into()])?;
                let storage_root = storage_proof_calculator
                    .compute_root_hash(&proof)?
                    .expect("storage_proof with dummy target always returns root");

                cached_storage_roots.insert(hashed_address, storage_root);

                drop(_guard);

                (account, storage_root)
            }
        };

        let account = account.into_trie_account(root);
        account.encode(buf);
        Ok(())
    }
}

/// Implements the [`LeafValueEncoder`] trait for accounts using a [`CrossbeamSender`] to dispatch
/// and compute storage roots asynchronously. Can also accept a set of already dispatched account
/// storage proofs, for cases where it's possible to determine some necessary accounts ahead of
/// time.
///
/// When the worker pool is busy (channel is full), falls back to synchronous storage root
/// computation using the provided cursor factories.
pub(crate) struct AsyncAccountValueEncoder<T, H> {
    /// Storage proof jobs which were dispatched ahead of time.
    dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
    /// Storage roots which have already been computed. This can be used only if a storage proof
    /// wasn't dispatched for an account, otherwise we must consume the proof result.
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    /// Tracks storage proof results received from the storage workers. [`Rc`] + [`RefCell`] is
    /// required because [`DeferredValueEncoder`] cannot have a lifetime.
    storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNode>>>>,
    /// Factory for creating trie cursors (for synchronous computation).
    trie_cursor_factory: Rc<T>,
    /// Factory for creating hashed cursors (for synchronous computation).
    hashed_cursor_factory: Rc<H>,
    /// Accumulated time spent waiting for storage proof results.
    storage_wait_time: Rc<RefCell<Duration>>,
}

impl<T, H> AsyncAccountValueEncoder<T, H> {
    /// Initializes a [`Self`] using cursor factories which will be used to calculate storage
    /// roots synchronously.
    ///
    /// # Parameters
    /// - `dispatched`: Pre-dispatched storage proof receivers for target accounts
    /// - `cached_storage_roots`: Shared cache of already-computed storage roots
    /// - `trie_cursor_factory`: Factory for creating trie cursors (for synchronous computation)
    /// - `hashed_cursor_factory`: Factory for creating hashed cursors (for synchronous computation)
    pub(crate) fn new(
        dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
        trie_cursor_factory: Rc<T>,
        hashed_cursor_factory: Rc<H>,
    ) -> Self {
        Self {
            dispatched,
            cached_storage_roots,
            storage_proof_results: Default::default(),
            trie_cursor_factory,
            hashed_cursor_factory,
            storage_wait_time: Default::default(),
        }
    }

    /// Consume [`Self`] and return all collected storage proofs which had been dispatched,
    /// along with the total time spent waiting for storage proofs.
    ///
    /// # Panics
    ///
    /// This method panics if any deferred encoders produced by [`Self::deferred_encoder`] have not
    /// been dropped.
    pub(crate) fn into_storage_proofs_with_wait_time(
        self,
    ) -> Result<(B256Map<Vec<ProofTrieNode>>, Duration), StateProofError> {
        let mut storage_proof_results = Rc::into_inner(self.storage_proof_results)
            .expect("no deferred encoders are still allocated")
            .into_inner();

        let mut storage_wait_time = Rc::into_inner(self.storage_wait_time)
            .expect("no deferred encoders are still allocated")
            .into_inner();

        // Any remaining dispatched proofs need to have their results collected
        for (hashed_address, rx) in &self.dispatched {
            let _guard = debug_span!(
                target: "trie::proof_task",
                "Waiting for storage proof",
                ?hashed_address,
            )
            .entered();
            let wait_start = Instant::now();
            let result = rx
                .recv()
                .map_err(|_| {
                    StateProofError::Database(DatabaseError::Other(format!(
                        "Storage proof channel closed for {hashed_address:?}",
                    )))
                })?
                .result?;
            storage_wait_time += wait_start.elapsed();
            drop(_guard);

            let StorageProofResult::V2 { proof, .. } = result else {
                panic!("StorageProofResult is not V2: {result:?}")
            };

            storage_proof_results.insert(*hashed_address, proof);
        }

        Ok((storage_proof_results, storage_wait_time))
    }
}

impl<T, H> LeafValueEncoder for AsyncAccountValueEncoder<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    type Value = Account;
    type DeferredEncoder = AsyncAccountDeferredValueEncoder<T, H>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        // If the proof job has already been dispatched for this account then it's not necessary to
        // dispatch another.
        if let Some(rx) = self.dispatched.remove(&hashed_address) {
            return AsyncAccountDeferredValueEncoder::Dispatched {
                hashed_address,
                account,
                proof_result_rx: Ok(rx),
                storage_proof_results: Some(self.storage_proof_results.clone()),
                storage_wait_time: self.storage_wait_time.clone(),
            }
        }

        // If the address didn't have a job dispatched for it then we can assume it has no targets,
        // and we only need its root.

        // If the root is already calculated then just use it directly
        if let Some(root) = self.cached_storage_roots.get(&hashed_address) {
            return AsyncAccountDeferredValueEncoder::FromCache { account, root: *root }
        }

        // Compute storage root synchronously
        AsyncAccountDeferredValueEncoder::Sync {
            trie_cursor_factory: self.trie_cursor_factory.clone(),
            hashed_cursor_factory: self.hashed_cursor_factory.clone(),
            hashed_address,
            account,
            cached_storage_roots: self.cached_storage_roots.clone(),
        }
    }
}
