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
    hashed_cursor::HashedStorageCursor,
    proof_v2::{DeferredValueEncoder, LeafValueEncoder, StorageProofCalculator},
    trie_cursor::TrieStorageCursor,
    ProofTrieNode,
};
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

/// Metrics collected by [`AsyncAccountValueEncoder`] during proof computation.
///
/// Tracks time spent waiting for storage proofs and counts of each deferred encoder variant used.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ValueEncoderMetrics {
    /// Accumulated time spent waiting for storage proof results from dispatched workers.
    pub(crate) storage_wait_time: Duration,
    /// Number of times the `Dispatched` variant was used (proof pre-dispatched to workers).
    pub(crate) dispatched_count: u64,
    /// Number of times the `FromCache` variant was used (storage root already cached).
    pub(crate) from_cache_count: u64,
    /// Number of times the `Sync` variant was used (synchronous computation).
    pub(crate) sync_count: u64,
}

impl ValueEncoderMetrics {
    /// Extends this metrics by adding the values from another.
    pub(crate) fn extend(&mut self, other: &Self) {
        self.storage_wait_time += other.storage_wait_time;
        self.dispatched_count += other.dispatched_count;
        self.from_cache_count += other.from_cache_count;
        self.sync_count += other.sync_count;
    }
}

/// Returned from [`AsyncAccountValueEncoder`], used to track an async storage root calculation.
pub(crate) enum AsyncAccountDeferredValueEncoder<TC, HC> {
    /// A storage proof job was dispatched to the worker pool.
    Dispatched {
        hashed_address: B256,
        account: Account,
        proof_result_rx: Result<CrossbeamReceiver<StorageProofResultMessage>, DatabaseError>,
        /// Shared storage proof results.
        storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNode>>>>,
        /// Shared metrics for tracking wait time and counts.
        metrics: Rc<RefCell<ValueEncoderMetrics>>,
    },
    /// The storage root was found in cache.
    FromCache { account: Account, root: B256 },
    /// Synchronous storage root computation.
    Sync {
        /// Shared storage proof calculator for computing storage roots.
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
        hashed_address: B256,
        account: Account,
        /// Cache to store computed storage roots for future reuse.
        cached_storage_roots: Arc<DashMap<B256, B256>>,
    },
}

impl<TC, HC> DeferredValueEncoder for AsyncAccountDeferredValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = alloy_primitives::U256>,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let (account, root) = match self {
            Self::Dispatched {
                hashed_address,
                account,
                proof_result_rx,
                storage_proof_results,
                metrics,
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
                metrics.borrow_mut().storage_wait_time += wait_start.elapsed();

                let StorageProofResult::V2 { root: Some(root), proof } = result else {
                    panic!("StorageProofResult is not V2 with root: {result:?}")
                };

                storage_proof_results.borrow_mut().insert(hashed_address, proof);

                (account, root)
            }
            Self::FromCache { account, root } => (account, root),
            Self::Sync { storage_calculator, hashed_address, account, cached_storage_roots } => {
                let mut calculator = storage_calculator.borrow_mut();
                let proof = calculator.storage_proof(hashed_address, &mut [B256::ZERO.into()])?;
                let storage_root = calculator
                    .compute_root_hash(&proof)?
                    .expect("storage_proof with dummy target always returns root");

                cached_storage_roots.insert(hashed_address, storage_root);
                (account, storage_root)
            }
        };

        let account = account.into_trie_account(root);
        account.encode(buf);
        Ok(())
    }
}

/// Implements the [`LeafValueEncoder`] trait for accounts.
///
/// Accepts a set of pre-dispatched storage proof receivers for accounts whose storage roots are
/// being computed asynchronously by worker threads.
///
/// For accounts without pre-dispatched proofs or cached roots, uses a shared
/// [`StorageProofCalculator`] to compute storage roots synchronously, reusing cursors across
/// multiple accounts.
pub(crate) struct AsyncAccountValueEncoder<TC, HC> {
    /// Storage proof jobs which were dispatched ahead of time.
    dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
    /// Storage roots which have already been computed. This can be used only if a storage proof
    /// wasn't dispatched for an account, otherwise we must consume the proof result.
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    /// Tracks storage proof results received from the storage workers. [`Rc`] + [`RefCell`] is
    /// required because [`DeferredValueEncoder`] cannot have a lifetime.
    storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNode>>>>,
    /// Shared storage proof calculator for synchronous computation. Reuses cursors and internal
    /// buffers across multiple storage root calculations.
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    /// Shared metrics for tracking wait time and variant counts.
    metrics: Rc<RefCell<ValueEncoderMetrics>>,
}

impl<TC, HC> AsyncAccountValueEncoder<TC, HC> {
    /// Initializes a [`Self`] using a storage proof calculator which will be reused to calculate
    /// storage roots synchronously.
    ///
    /// # Parameters
    /// - `dispatched`: Pre-dispatched storage proof receivers for target accounts
    /// - `cached_storage_roots`: Shared cache of already-computed storage roots
    /// - `storage_calculator`: Shared storage proof calculator for synchronous computation
    pub(crate) fn new(
        dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    ) -> Self {
        Self {
            dispatched,
            cached_storage_roots,
            storage_proof_results: Default::default(),
            storage_calculator,
            metrics: Default::default(),
        }
    }

    /// Consume [`Self`] and return all collected storage proofs along with accumulated metrics.
    ///
    /// This method collects any remaining dispatched proofs that weren't consumed during proof
    /// calculation and includes their wait time in the returned metrics.
    ///
    /// # Panics
    ///
    /// This method panics if any deferred encoders produced by [`Self::deferred_encoder`] have not
    /// been dropped.
    pub(crate) fn finalize(
        self,
    ) -> Result<(B256Map<Vec<ProofTrieNode>>, ValueEncoderMetrics), StateProofError> {
        let mut storage_proof_results = Rc::into_inner(self.storage_proof_results)
            .expect("no deferred encoders are still allocated")
            .into_inner();

        let mut metrics = Rc::into_inner(self.metrics)
            .expect("no deferred encoders are still allocated")
            .into_inner();

        // Any remaining dispatched proofs need to have their results collected.
        // These are proofs that were pre-dispatched but not consumed during proof calculation.
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
            metrics.storage_wait_time += wait_start.elapsed();

            let StorageProofResult::V2 { proof, .. } = result else {
                panic!("StorageProofResult is not V2: {result:?}")
            };

            storage_proof_results.insert(*hashed_address, proof);
        }

        Ok((storage_proof_results, metrics))
    }
}

impl<TC, HC> LeafValueEncoder for AsyncAccountValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = alloy_primitives::U256>,
{
    type Value = Account;
    type DeferredEncoder = AsyncAccountDeferredValueEncoder<TC, HC>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        // If the proof job has already been dispatched for this account then it's not necessary to
        // dispatch another.
        if let Some(rx) = self.dispatched.remove(&hashed_address) {
            self.metrics.borrow_mut().dispatched_count += 1;
            return AsyncAccountDeferredValueEncoder::Dispatched {
                hashed_address,
                account,
                proof_result_rx: Ok(rx),
                storage_proof_results: self.storage_proof_results.clone(),
                metrics: self.metrics.clone(),
            }
        }

        // If the address didn't have a job dispatched for it then we can assume it has no targets,
        // and we only need its root.

        // If the root is already calculated then just use it directly
        if let Some(root) = self.cached_storage_roots.get(&hashed_address) {
            self.metrics.borrow_mut().from_cache_count += 1;
            return AsyncAccountDeferredValueEncoder::FromCache { account, root: *root }
        }

        // Compute storage root synchronously using the shared calculator
        self.metrics.borrow_mut().sync_count += 1;
        AsyncAccountDeferredValueEncoder::Sync {
            storage_calculator: self.storage_calculator.clone(),
            hashed_address,
            account,
            cached_storage_roots: self.cached_storage_roots.clone(),
        }
    }
}
