use crate::proof_task::{StorageProofResult, StorageProofResultMessage};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use alloy_rlp::Encodable;
use core::cell::RefCell;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::{dashmap::DashMap, Account};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedStorageCursor,
    proof_v2::{DeferredValueEncoder, LeafValueEncoder, StorageProofCalculator},
    trie_cursor::TrieStorageCursor,
    ProofTrieNodeV2,
};
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

/// Stats collected by [`AsyncAccountValueEncoder`] during proof computation.
///
/// Tracks time spent waiting for storage proofs and counts of each deferred encoder variant used.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ValueEncoderStats {
    /// Accumulated time spent waiting for storage proof results from dispatched workers.
    pub(crate) storage_wait_time: Duration,
    /// Number of times the `Dispatched` variant was used (proof pre-dispatched to workers).
    pub(crate) dispatched_count: u64,
    /// Number of times the `FromCache` variant was used (storage root already cached).
    pub(crate) from_cache_count: u64,
    /// Number of times the `Sync` variant was used (synchronous computation).
    pub(crate) sync_count: u64,
    /// Number of times a dispatched storage proof had no root node and fell back to sync
    /// computation.
    pub(crate) dispatched_missing_root_count: u64,
}

impl ValueEncoderStats {
    /// Extends this metrics by adding the values from another.
    pub(crate) fn extend(&mut self, other: &Self) {
        self.storage_wait_time += other.storage_wait_time;
        self.dispatched_count += other.dispatched_count;
        self.from_cache_count += other.from_cache_count;
        self.sync_count += other.sync_count;
        self.dispatched_missing_root_count += other.dispatched_missing_root_count;
    }
}

/// Shared receiver for storage proof jobs dispatched by one account multiproof.
#[derive(Debug)]
pub(crate) struct DispatchedStorageProofs {
    /// Addresses whose dispatched proof can still be claimed by account value encoding.
    claimable: B256Set,
    /// Addresses whose dispatched proof has not yet been received.
    pending: B256Set,
    /// Storage worker result receiver shared by all dispatched storage jobs for this multiproof.
    receiver: CrossbeamReceiver<StorageProofResultMessage>,
    /// Results received while waiting for another address.
    completed: B256Map<StorageProofResult>,
}

impl DispatchedStorageProofs {
    /// Creates shared storage proof receiver state.
    pub(crate) fn new(
        addresses: B256Set,
        receiver: CrossbeamReceiver<StorageProofResultMessage>,
    ) -> Self {
        Self {
            claimable: addresses.clone(),
            pending: addresses,
            receiver,
            completed: B256Map::default(),
        }
    }

    /// Marks `hashed_address` as owned by a deferred value encoder.
    fn claim(&mut self, hashed_address: &B256) -> bool {
        self.claimable.remove(hashed_address)
    }

    /// Receives storage proof results until the requested address is available.
    fn receive_for(
        &mut self,
        hashed_address: B256,
        stats: &mut ValueEncoderStats,
    ) -> Result<StorageProofResult, StateProofError> {
        if let Some(result) = self.completed.remove(&hashed_address) {
            return Ok(result);
        }

        let wait_start = Instant::now();
        loop {
            let msg = self.receiver.recv().map_err(|_| {
                StateProofError::Database(DatabaseError::Other(format!(
                    "Storage proof channel closed for {hashed_address:?}",
                )))
            })?;

            self.pending.remove(&msg.hashed_address);
            let msg_address = msg.hashed_address;
            let result = msg.result?;

            if msg_address == hashed_address {
                stats.storage_wait_time += wait_start.elapsed();
                return Ok(result);
            }

            self.completed.insert(msg_address, result);
        }
    }

    /// Collects all remaining storage proof results into `storage_proof_results`.
    fn drain_remaining(
        &mut self,
        storage_proof_results: &mut B256Map<Vec<ProofTrieNodeV2>>,
        stats: &mut ValueEncoderStats,
    ) -> Result<(), StateProofError> {
        for (hashed_address, result) in self.completed.drain() {
            storage_proof_results.insert(hashed_address, result.proof);
        }

        if self.pending.is_empty() {
            return Ok(());
        }

        let wait_start = Instant::now();
        while !self.pending.is_empty() {
            let msg = self.receiver.recv().map_err(|_| {
                StateProofError::Database(DatabaseError::Other(
                    "Storage proof channel closed while draining remaining proofs".to_string(),
                ))
            })?;

            self.pending.remove(&msg.hashed_address);
            let result = msg.result?;
            storage_proof_results.insert(msg.hashed_address, result.proof);
        }
        stats.storage_wait_time += wait_start.elapsed();

        Ok(())
    }
}

/// Returned from [`AsyncAccountValueEncoder`], used to track an async storage root calculation.
pub(crate) enum AsyncAccountDeferredValueEncoder<TC, HC> {
    /// A storage proof job was dispatched to the worker pool.
    Dispatched {
        hashed_address: B256,
        account: Account,
        /// Shared storage proof result receiver.
        dispatched_storage_proofs: Rc<RefCell<DispatchedStorageProofs>>,
        /// Shared storage proof results.
        storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNodeV2>>>>,
        /// Shared stats for tracking wait time and counts.
        stats: Rc<RefCell<ValueEncoderStats>>,
        /// Shared storage proof calculator for synchronous fallback when dispatched proof has no
        /// root.
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
        /// Cache to store computed storage roots for future reuse.
        cached_storage_roots: Arc<DashMap<B256, B256>>,
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
    fn encode(mut self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let (account, root) = match &mut self {
            Self::Dispatched {
                hashed_address,
                account,
                dispatched_storage_proofs,
                storage_proof_results,
                stats,
                storage_calculator,
                cached_storage_roots,
            } => {
                let hashed_address = *hashed_address;
                let account = *account;
                let result = {
                    let mut dispatched_storage_proofs = dispatched_storage_proofs.borrow_mut();
                    let mut stats = stats.borrow_mut();
                    dispatched_storage_proofs.receive_for(hashed_address, &mut stats)?
                };

                storage_proof_results.borrow_mut().insert(hashed_address, result.proof);

                let root = match result.root {
                    Some(root) => root,
                    None => {
                        // In `compute_v2_account_multiproof` we ensure that all dispatched storage
                        // proofs computations for which there is also an account proof will return
                        // a root node, but it could happen randomly that an account which is not in
                        // the account proof targets, but _is_ in storage proof targets, will need
                        // to be encoded as part of general trie traversal, so we need to handle
                        // that case here.
                        stats.borrow_mut().dispatched_missing_root_count += 1;

                        let mut calculator = storage_calculator.borrow_mut();
                        let root_node = calculator.storage_root_node(hashed_address)?;
                        let storage_root = calculator
                            .compute_root_hash(&[root_node])?
                            .expect("storage_root_node returns a node at empty path");

                        cached_storage_roots.insert(hashed_address, storage_root);
                        storage_root
                    }
                };

                (account, root)
            }
            Self::FromCache { account, root } => (*account, *root),
            Self::Sync { storage_calculator, hashed_address, account, cached_storage_roots } => {
                let hashed_address = *hashed_address;
                let account = *account;
                let mut calculator = storage_calculator.borrow_mut();
                let root_node = calculator.storage_root_node(hashed_address)?;
                let storage_root = calculator
                    .compute_root_hash(&[root_node])?
                    .expect("storage_root_node returns a node at empty path");

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
    dispatched: Option<Rc<RefCell<DispatchedStorageProofs>>>,
    /// Storage roots which have already been computed. This can be used only if a storage proof
    /// wasn't dispatched for an account, otherwise we must consume the proof result.
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    /// Tracks storage proof results received from the storage workers. [`Rc`] + [`RefCell`] is
    /// required because [`DeferredValueEncoder`] cannot have a lifetime.
    storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNodeV2>>>>,
    /// Shared storage proof calculator for synchronous computation. Reuses cursors and internal
    /// buffers across multiple storage root calculations.
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    /// Shared stats for tracking wait time and variant counts.
    stats: Rc<RefCell<ValueEncoderStats>>,
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
        dispatched: Option<DispatchedStorageProofs>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    ) -> Self {
        Self {
            dispatched: dispatched.map(|dispatched| Rc::new(RefCell::new(dispatched))),
            cached_storage_roots,
            storage_proof_results: Default::default(),
            storage_calculator,
            stats: Default::default(),
        }
    }

    /// Consume [`Self`] and return all collected storage proofs along with accumulated stats.
    ///
    /// This method collects any remaining dispatched proofs that weren't consumed during proof
    /// calculation and includes their wait time in the returned stats.
    ///
    /// # Panics
    ///
    /// This method panics if any deferred encoders produced by [`Self::deferred_encoder`] have not
    /// been dropped.
    pub(crate) fn finalize(
        self,
    ) -> Result<(B256Map<Vec<ProofTrieNodeV2>>, ValueEncoderStats), StateProofError> {
        let mut storage_proof_results = Rc::into_inner(self.storage_proof_results)
            .expect("no deferred encoders are still allocated")
            .into_inner();

        let mut stats = Rc::into_inner(self.stats)
            .expect("no deferred encoders are still allocated")
            .into_inner();

        if let Some(dispatched) = self.dispatched {
            let mut dispatched = Rc::into_inner(dispatched)
                .expect("no deferred encoders are still allocated")
                .into_inner();
            dispatched.drain_remaining(&mut storage_proof_results, &mut stats)?;
        }

        Ok((storage_proof_results, stats))
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
        if let Some(dispatched) = &self.dispatched
            && dispatched.borrow_mut().claim(&hashed_address)
        {
            self.stats.borrow_mut().dispatched_count += 1;
            return AsyncAccountDeferredValueEncoder::Dispatched {
                hashed_address,
                account,
                dispatched_storage_proofs: dispatched.clone(),
                storage_proof_results: self.storage_proof_results.clone(),
                stats: self.stats.clone(),
                storage_calculator: self.storage_calculator.clone(),
                cached_storage_roots: self.cached_storage_roots.clone(),
            };
        }

        // If the address didn't have a job dispatched for it then we can assume it has no targets,
        // and we only need its root.

        // If the root is already calculated then just use it directly
        if let Some(root) = self.cached_storage_roots.get(&hashed_address) {
            self.stats.borrow_mut().from_cache_count += 1;
            return AsyncAccountDeferredValueEncoder::FromCache { account, root: *root };
        }

        // Compute storage root synchronously using the shared calculator
        self.stats.borrow_mut().sync_count += 1;
        AsyncAccountDeferredValueEncoder::Sync {
            storage_calculator: self.storage_calculator.clone(),
            hashed_address,
            account,
            cached_storage_roots: self.cached_storage_roots.clone(),
        }
    }
}
