use crate::proof_task::{
    StorageProofInput, StorageProofResult, StorageProofResultMessage, StorageWorkerJob,
};
use alloy_primitives::{map::B256Map, B256};
use alloy_rlp::Encodable;
use core::cell::RefCell;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use dashmap::DashMap;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    proof_v2::{self, DeferredValueEncoder, LeafValueEncoder, Target},
    trie_cursor::TrieCursorFactory,
    ProofTrieNode,
};
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

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
    /// Fall back to synchronous storage root computation when the worker pool is busy.
    Sync {
        trie_cursor_factory: Rc<T>,
        hashed_cursor_factory: Rc<H>,
        hashed_address: B256,
        account: Account,
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

                let StorageProofResult::V2 { root: Some(root), proof } = result else {
                    panic!("StorageProofResult is not V2 with root: {result:?}")
                };

                if let Some(storage_proof_results) = storage_proof_results.as_ref() {
                    storage_proof_results.borrow_mut().insert(hashed_address, proof);
                }

                (account, root)
            }
            Self::FromCache { account, root } => (account, root),
            Self::Sync { trie_cursor_factory, hashed_cursor_factory, hashed_address, account } => {
                let trie_cursor = trie_cursor_factory.storage_trie_cursor(hashed_address)?;
                let hashed_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

                let mut storage_proof_calculator =
                    proof_v2::ProofCalculator::new_storage(trie_cursor, hashed_cursor);

                let proof = storage_proof_calculator
                    .storage_proof(hashed_address, &mut [B256::ZERO.into()])?;
                let storage_root = storage_proof_calculator
                    .compute_root_hash(&proof)?
                    .expect("storage_proof with dummy target always returns root");

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
    /// Channel for dispatching on-demand storage root proofs during account iteration.
    /// This uses a separate worker pool to avoid blocking behind pre-dispatched target proofs.
    /// The channel is bounded to the number of workers.
    account_storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Factory for creating trie cursors (for synchronous fallback).
    trie_cursor_factory: Rc<T>,
    /// Factory for creating hashed cursors (for synchronous fallback).
    hashed_cursor_factory: Rc<H>,
    /// Accumulated time spent waiting for storage proof results.
    storage_wait_time: Rc<RefCell<Duration>>,
}

impl<T, H> AsyncAccountValueEncoder<T, H> {
    /// Initializes a [`Self`] using cursor factories which will be used to calculate storage
    /// roots synchronously when the worker pool is busy.
    ///
    /// # Parameters
    /// - `dispatched`: Pre-dispatched storage proof receivers for target accounts
    /// - `cached_storage_roots`: Shared cache of already-computed storage roots
    /// - `account_storage_work_tx`: Bounded channel for on-demand storage root proofs (uses
    ///   separate worker pool to avoid blocking behind pre-dispatched target proofs)
    /// - `trie_cursor_factory`: Factory for creating trie cursors (for synchronous fallback)
    /// - `hashed_cursor_factory`: Factory for creating hashed cursors (for synchronous fallback)
    pub(crate) fn new(
        dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
        account_storage_work_tx: CrossbeamSender<StorageWorkerJob>,
        trie_cursor_factory: Rc<T>,
        hashed_cursor_factory: Rc<H>,
    ) -> Self {
        Self {
            dispatched,
            cached_storage_roots,
            storage_proof_results: Default::default(),
            account_storage_work_tx,
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

        // Create a proof input which targets a bogus key, so that we calculate the root as a
        // side-effect. Use the account storage worker pool to avoid blocking behind pre-dispatched
        // target storage proofs.
        let input = StorageProofInput::new(hashed_address, vec![Target::new(B256::ZERO)]);
        let (tx, rx) = crossbeam_channel::bounded(1);

        // Try to dispatch to the worker pool. If the channel is full, fall back to synchronous
        // computation to avoid blocking.
        match self
            .account_storage_work_tx
            .try_send(StorageWorkerJob::StorageProof { input, proof_result_sender: tx })
        {
            Ok(()) => AsyncAccountDeferredValueEncoder::Dispatched {
                hashed_address,
                account,
                proof_result_rx: Ok(rx),
                storage_proof_results: None,
                storage_wait_time: self.storage_wait_time.clone(),
            },
            Err(
                crossbeam_channel::TrySendError::Full(_) |
                crossbeam_channel::TrySendError::Disconnected(_),
            ) => {
                // Channel is full or workers are disabled, fall back to synchronous computation
                AsyncAccountDeferredValueEncoder::Sync {
                    trie_cursor_factory: self.trie_cursor_factory.clone(),
                    hashed_cursor_factory: self.hashed_cursor_factory.clone(),
                    hashed_address,
                    account,
                }
            }
        }
    }
}
