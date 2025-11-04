//! Parallel proof computation using dynamic task spawning with short-lived database transactions.
//!
//! # Architecture
//!
//! - **Dynamic Task Spawning**: Tasks are spawned on-demand using the Tokio runtime
//!   - Storage tasks: Handle storage proofs and blinded storage node requests
//!   - Account tasks: Handle account multiproofs and blinded account node requests
//! - **Semaphore-Based Backpressure**: Limits concurrent tasks to prevent memory exhaustion
//! - **Short-Lived Transactions**: Database transactions are acquired per-task and released immediately
//! - **Work-Stealing Scheduler**: Tokio's work-stealing optimizes CPU utilization
//!
//! # Message Flow
//!
//! 1. `MultiProofTask` prepares a storage or account job and dispatches it via [`ProofWorkerHandle`].
//!    The job carries a [`ProofResultContext`] so the task knows how to send the result back.
//! 2. A dynamic task is spawned with a semaphore permit, acquires a database transaction, computes
//!    the proof, and sends a [`ProofResultMessage`] through the provided [`ProofResultSender`].
//! 3. The transaction is dropped immediately, allowing MDBX to reclaim pages.
//! 4. `MultiProofTask` receives the message, uses `sequence_number` to keep proofs in order, and
//!    proceeds with its state-root logic.
//!
//! Each job gets its own direct channel so results go straight back to `MultiProofTask`. Tasks are
//! independent and self-contained, spawning only when needed.
//!
//! ```
//! MultiProofTask -> MultiproofManager -> ProofWorkerHandle -> Tokio Runtime
//!        ^                                          |
//!        |                                          v
//!        |                              Dynamic Task (spawn_blocking)
//!        |                              ├─ Acquire semaphore permit
//!        |                              ├─ Acquire DB transaction
//!        |                              ├─ Compute proof
//!        |                              ├─ Send result
//!        |                              └─ Drop transaction (MDBX reclaims pages)
//!        |                                          |
//!        └──────────────────────────────────────────┘
//!                   ProofResultMessage
//! ```
use crate::{
    root::ParallelStateRootError,
    stats::{ParallelTrieStats, ParallelTrieTracker},
    StorageRootTargets,
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use alloy_rlp::{BufMut, Encodable};
// use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use dashmap::DashMap;
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind};
use reth_provider::{DatabaseProviderROFactory, ProviderError};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{TriePrefixSets, TriePrefixSetsMut},
    proof::{ProofBlindedAccountProvider, ProofBlindedStorageProvider, StorageProof},
    trie_cursor::TrieCursorFactory,
    walker::TrieWalker,
    DecodedMultiProof, DecodedStorageMultiProof, HashBuilder, HashedPostState, MultiProofTargets,
    Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSet, PrefixSetMut},
    proof::{DecodedProofNodes, ProofRetainer},
};
use reth_trie_sparse::provider::{RevealedNode, TrieNodeProvider, TrieNodeProviderFactory};
use std::{
    sync::{
        mpsc::{channel, Receiver},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::Semaphore};
use tracing::{debug, debug_span, error, trace};

type StorageProofResult = Result<DecodedStorageMultiProof, ParallelStateRootError>;
type TrieNodeProviderResult = Result<Option<RevealedNode>, SparseTrieError>;

/// A handle for dynamically spawning proof computation tasks.
///
/// This handle uses Tokio's runtime to spawn async tasks on-demand, with semaphores
/// providing backpressure. Each task acquires a short-lived database transaction,
/// computes the proof, and releases the transaction immediately for MDBX page reclamation.
/// All handles share reference-counted semaphores and task context.
#[derive(Clone)]
pub struct ProofWorkerHandle<Factory> {
    /// Tokio runtime handle for spawning tasks
    executor: Handle,
    /// Shared context with database factory and prefix sets
    task_ctx: Arc<ProofTaskCtx<Factory>>,
    /// Semaphore limiting concurrent storage proof tasks
    storage_semaphore: Arc<Semaphore>,
    /// Semaphore limiting concurrent account proof tasks
    account_semaphore: Arc<Semaphore>,
}

impl<Factory> std::fmt::Debug for ProofWorkerHandle<Factory> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProofWorkerHandle")
            .field("executor", &"<Handle>")
            .field("task_ctx", &"<Arc<ProofTaskCtx>>")
            .field("storage_permits", &self.storage_semaphore.available_permits())
            .field("account_permits", &self.account_semaphore.available_permits())
            .finish()
    }
}

impl<Factory> ProofWorkerHandle<Factory>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Creates a new handle for dynamic proof task spawning.
    ///
    /// # Parameters
    /// - `executor`: Tokio runtime handle
    /// - `task_ctx`: Shared context with database factory
    /// - `max_storage_tasks`: Maximum concurrent storage proof tasks
    /// - `max_account_tasks`: Maximum concurrent account proof tasks
    pub fn new(
        executor: Handle,
        task_ctx: ProofTaskCtx<Factory>,
        max_storage_tasks: usize,
        max_account_tasks: usize,
    ) -> Self {
        debug!(
            target: "trie::proof_task",
            max_storage_tasks,
            max_account_tasks,
            "Initializing dynamic proof worker handle"
        );

        Self {
            executor,
            task_ctx: Arc::new(task_ctx),
            storage_semaphore: Arc::new(Semaphore::new(max_storage_tasks)),
            account_semaphore: Arc::new(Semaphore::new(max_account_tasks)),
        }
    }

    /// Dispatch a storage proof computation as a dynamic task
    pub fn dispatch_storage_proof(
        &self,
        input: StorageProofInput,
        proof_result_sender: ProofResultContext,
    ) -> Result<(), ProviderError> {
        let task_ctx = self.task_ctx.clone();
        let semaphore = self.storage_semaphore.clone();

        self.executor.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            tokio::task::spawn_blocking(move || {
                let span = debug_span!(
                    target: "trie::proof_task",
                    "Storage proof task",
                    hashed_address = ?input.hashed_address,
                );
                let _guard = span.enter();

                // Acquire transaction FOR THIS TASK ONLY
                let provider = match task_ctx.factory.database_provider_ro() {
                    Ok(p) => p,
                    Err(e) => {
                        error!(target: "trie::proof_task", "Failed to acquire provider: {e}");
                        let _ = proof_result_sender.sender.send(ProofResultMessage {
                            sequence_number: proof_result_sender.sequence_number,
                            result: Err(ParallelStateRootError::Provider(e)),
                            elapsed: proof_result_sender.start_time.elapsed(),
                            state: proof_result_sender.state,
                        });
                        return;
                    }
                };

                let proof_tx = ProofTaskTx::new(provider, task_ctx.prefix_sets.clone(), 0);

                trace!(target: "trie::proof_task", "Computing storage proof");

                let hashed_address = input.hashed_address;
                let result = proof_tx.compute_storage_proof(input);

                let result_msg = result.map(|storage_proof| ProofResult::StorageProof {
                    hashed_address,
                    proof: storage_proof,
                });

                if proof_result_sender
                    .sender
                    .send(ProofResultMessage {
                        sequence_number: proof_result_sender.sequence_number,
                        result: result_msg,
                        elapsed: proof_result_sender.start_time.elapsed(),
                        state: proof_result_sender.state,
                    })
                    .is_err()
                {
                    trace!(target: "trie::proof_task", "Proof result receiver dropped");
                }

                // provider drops here → transaction closes → MDBX reclaims pages
            })
            .await
            .unwrap();
        });

        Ok(())
    }

    /// Dispatch an account multiproof computation
    ///
    /// The result will be sent via the `result_sender` channel included in the input.
    pub fn dispatch_account_multiproof(
        &self,
        input: AccountMultiproofInput,
    ) -> Result<(), ProviderError> {
        let task_ctx = self.task_ctx.clone();
        let semaphore = self.account_semaphore.clone();
        let storage_handle = self.clone();

        self.executor.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            tokio::task::spawn_blocking(move || {
                let AccountMultiproofInput {
                    targets,
                    mut prefix_sets,
                    collect_branch_node_masks,
                    multi_added_removed_keys,
                    missed_leaves_storage_roots,
                    proof_result_sender:
                        ProofResultContext {
                            sender: result_tx,
                            sequence_number: seq,
                            state,
                            start_time: start,
                        },
                } = input;

                let span = debug_span!(
                    target: "trie::proof_task",
                    "Account multiproof task",
                    targets = targets.len(),
                );
                let _guard = span.enter();

                // Acquire provider for this task
                let provider = match task_ctx.factory.database_provider_ro() {
                    Ok(p) => p,
                    Err(e) => {
                        error!(target: "trie::proof_task", "Failed to acquire provider: {e}");
                        let _ = result_tx.send(ProofResultMessage {
                            sequence_number: seq,
                            result: Err(ParallelStateRootError::Provider(e)),
                            elapsed: start.elapsed(),
                            state,
                        });
                        return;
                    }
                };

                let proof_tx = ProofTaskTx::new(provider, task_ctx.prefix_sets.clone(), 0);

                trace!(target: "trie::proof_task", "Computing account multiproof");

                let mut tracker = ParallelTrieTracker::default();
                let mut storage_prefix_sets = std::mem::take(&mut prefix_sets.storage_prefix_sets);
                let storage_root_targets_len =
                    StorageRootTargets::count(&prefix_sets.account_prefix_set, &storage_prefix_sets);
                tracker.set_precomputed_storage_roots(storage_root_targets_len as u64);

                // Dispatch storage proofs dynamically
                let storage_proof_receivers = match dispatch_storage_proofs_dynamic(
                    &storage_handle,
                    &targets,
                    &mut storage_prefix_sets,
                    collect_branch_node_masks,
                    multi_added_removed_keys.as_ref(),
                ) {
                    Ok(receivers) => receivers,
                    Err(error) => {
                        error!(target: "trie::proof_task", "Failed to dispatch storage proofs: {error}");
                        let _ = result_tx.send(ProofResultMessage {
                            sequence_number: seq,
                            result: Err(error),
                            elapsed: start.elapsed(),
                            state,
                        });
                        return;
                    }
                };

                let account_prefix_set = std::mem::take(&mut prefix_sets.account_prefix_set);

                let ctx = AccountMultiproofParams {
                    targets: &targets,
                    prefix_set: account_prefix_set,
                    collect_branch_node_masks,
                    multi_added_removed_keys: multi_added_removed_keys.as_ref(),
                    storage_proof_receivers,
                    missed_leaves_storage_roots: missed_leaves_storage_roots.as_ref(),
                };

                let result = build_account_multiproof_with_storage_roots(
                    &proof_tx.provider,
                    ctx,
                    &mut tracker,
                );

                let stats = tracker.finish();
                let result = result.map(|proof| ProofResult::AccountMultiproof { proof, stats });

                if result_tx
                    .send(ProofResultMessage {
                        sequence_number: seq,
                        result,
                        elapsed: start.elapsed(),
                        state,
                    })
                    .is_err()
                {
                    trace!(target: "trie::proof_task", "Account multiproof receiver dropped");
                }

                // provider drops → transaction closes
            })
            .await
            .unwrap();
        });

        Ok(())
    }

    /// Dispatch blinded storage node request to storage worker pool
    pub(crate) fn dispatch_blinded_storage_node(
        &self,
        account: B256,
        path: Nibbles,
    ) -> Result<Receiver<TrieNodeProviderResult>, ProviderError> {
        let (tx, rx) = channel();
        let task_ctx = self.task_ctx.clone();
        let semaphore = self.storage_semaphore.clone();

        self.executor.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            tokio::task::spawn_blocking(move || {
                let provider = match task_ctx.factory.database_provider_ro() {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(Err(SparseTrieErrorKind::Other(Box::new(e)).into()));
                        return;
                    }
                };

                let proof_tx = ProofTaskTx::new(provider, task_ctx.prefix_sets.clone(), 0);
                let result = proof_tx.process_blinded_storage_node(account, &path);
                let _ = tx.send(result);
            })
            .await
            .unwrap();
        });

        Ok(rx)
    }

    /// Dispatch blinded storage node request as a dynamic task
    pub(crate) fn dispatch_blinded_account_node(
        &self,
        path: Nibbles,
    ) -> Result<Receiver<TrieNodeProviderResult>, ProviderError> {
        let (tx, rx) = channel();
        let task_ctx = self.task_ctx.clone();
        let semaphore = self.account_semaphore.clone();

        self.executor.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            tokio::task::spawn_blocking(move || {
                let provider = match task_ctx.factory.database_provider_ro() {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(Err(SparseTrieErrorKind::Other(Box::new(e)).into()));
                        return;
                    }
                };

                let proof_tx = ProofTaskTx::new(provider, task_ctx.prefix_sets.clone(), 0);
                let result = proof_tx.process_blinded_account_node(&path);
                let _ = tx.send(result);
            })
            .await
            .unwrap();
        });

        Ok(rx)
    }
}

/// Data used for initializing cursor factories that is shared across all storage proof instances.
#[derive(Clone, Debug)]
pub struct ProofTaskCtx<Factory> {
    /// The factory for creating state providers.
    factory: Factory,
    /// The collection of prefix sets for the computation. Since the prefix sets _always_
    /// invalidate the in-memory nodes, not all keys from `state_sorted` might be present here,
    /// if we have cached nodes for them.
    prefix_sets: Arc<TriePrefixSetsMut>,
}

impl<Factory> ProofTaskCtx<Factory> {
    /// Creates a new [`ProofTaskCtx`] with the given factory and prefix sets.
    pub const fn new(factory: Factory, prefix_sets: Arc<TriePrefixSetsMut>) -> Self {
        Self { factory, prefix_sets }
    }
}

/// This contains all information shared between all storage proof instances.
#[derive(Debug)]
pub struct ProofTaskTx<Provider> {
    /// The provider that implements `TrieCursorFactory` and `HashedCursorFactory`.
    provider: Provider,

    /// The prefix sets for the computation.
    prefix_sets: Arc<TriePrefixSetsMut>,

    /// Identifier for the worker within the worker pool, used only for tracing.
    id: usize,
}

impl<Provider> ProofTaskTx<Provider> {
    /// Initializes a [`ProofTaskTx`] with the given provider, prefix sets, and ID.
    const fn new(provider: Provider, prefix_sets: Arc<TriePrefixSetsMut>, id: usize) -> Self {
        Self { provider, prefix_sets, id }
    }
}

impl<Provider> ProofTaskTx<Provider>
where
    Provider: TrieCursorFactory + HashedCursorFactory,
{
    /// Compute storage proof.
    ///
    /// Used by dynamically spawned tasks to compute storage proofs with short-lived transactions.
    #[inline]
    //slight
    fn compute_storage_proof(&self, input: StorageProofInput) -> StorageProofResult {
        // Consume the input so we can move large collections (e.g. target slots) without cloning.
        let StorageProofInput {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
        } = input;

        // Get or create added/removed keys context
        let multi_added_removed_keys =
            multi_added_removed_keys.unwrap_or_else(|| Arc::new(MultiAddedRemovedKeys::new()));
        let added_removed_keys = multi_added_removed_keys.get_storage(&hashed_address);

        let span = debug_span!(
            target: "trie::proof_task",
            "Storage proof calculation",
            hashed_address = ?hashed_address,
            worker_id = self.id,
        );
        let _span_guard = span.enter();

        let proof_start = Instant::now();

        // Compute raw storage multiproof
        let raw_proof_result =
            StorageProof::new_hashed(&self.provider, &self.provider, hashed_address)
                .with_prefix_set_mut(PrefixSetMut::from(prefix_set.iter().copied()))
                .with_branch_node_masks(with_branch_node_masks)
                .with_added_removed_keys(added_removed_keys)
                .storage_multiproof(target_slots)
                .map_err(|e| ParallelStateRootError::Other(e.to_string()));

        // Decode proof into DecodedStorageMultiProof
        let decoded_result = raw_proof_result.and_then(|raw_proof| {
            raw_proof.try_into().map_err(|e: alloy_rlp::Error| {
                ParallelStateRootError::Other(format!(
                    "Failed to decode storage proof for {}: {}",
                    hashed_address, e
                ))
            })
        });

        trace!(
            target: "trie::proof_task",
            hashed_address = ?hashed_address,
            proof_time_us = proof_start.elapsed().as_micros(),
            worker_id = self.id,
            "Completed storage proof calculation"
        );

        decoded_result
    }

    /// Process a blinded storage node request.
    ///
    /// Used by dynamic tasks to retrieve blinded storage trie nodes for proof construction.
    fn process_blinded_storage_node(
        &self,
        account: B256,
        path: &Nibbles,
    ) -> TrieNodeProviderResult {
        let storage_node_provider = ProofBlindedStorageProvider::new(
            &self.provider,
            &self.provider,
            self.prefix_sets.clone(),
            account,
        );
        storage_node_provider.trie_node(path)
    }

    /// Process a blinded account node request.
    ///
    /// Used by dynamic tasks to retrieve blinded account trie nodes for proof construction.
    fn process_blinded_account_node(&self, path: &Nibbles) -> TrieNodeProviderResult {
        let account_node_provider = ProofBlindedAccountProvider::new(
            &self.provider,
            &self.provider,
            self.prefix_sets.clone(),
        );
        account_node_provider.trie_node(path)
    }
}

impl<Factory> TrieNodeProviderFactory for ProofWorkerHandle<Factory>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    type AccountNodeProvider = ProofTaskTrieNodeProvider<Factory>;
    type StorageNodeProvider = ProofTaskTrieNodeProvider<Factory>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofTaskTrieNodeProvider::AccountNode { handle: self.clone() }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofTaskTrieNodeProvider::StorageNode { account, handle: self.clone() }
    }
}

/// Trie node provider for retrieving trie nodes by path.
#[derive(Debug)]
pub enum ProofTaskTrieNodeProvider<Factory> {
    /// Blinded account trie node provider.
    AccountNode {
        /// Handle for spawning dynamic proof tasks.
        handle: ProofWorkerHandle<Factory>,
    },
    /// Blinded storage trie node provider.
    StorageNode {
        /// Target account.
        account: B256,
        /// Handle for spawning dynamic proof tasks.
        handle: ProofWorkerHandle<Factory>,
    },
}

impl<Factory> TrieNodeProvider for ProofTaskTrieNodeProvider<Factory>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    fn trie_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        match self {
            Self::AccountNode { handle } => {
                let rx = handle
                    .dispatch_blinded_account_node(*path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
            Self::StorageNode { handle, account } => {
                let rx = handle
                    .dispatch_blinded_storage_node(*account, *path)
                    .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
                rx.recv().map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?
            }
        }
    }
}

/// Result of a proof calculation, which can be either an account multiproof or a storage proof.
#[derive(Debug)]
pub enum ProofResult {
    /// Account multiproof with statistics
    AccountMultiproof {
        /// The account multiproof
        proof: DecodedMultiProof,
        /// Statistics collected during proof computation
        stats: ParallelTrieStats,
    },
    /// Storage proof for a specific account
    StorageProof {
        /// The hashed address this storage proof belongs to
        hashed_address: B256,
        /// The storage multiproof
        proof: DecodedStorageMultiProof,
    },
}

impl ProofResult {
    /// Convert this proof result into a `DecodedMultiProof`.
    ///
    /// For account multiproofs, returns the multiproof directly (discarding stats).
    /// For storage proofs, wraps the storage proof into a minimal multiproof.
    pub fn into_multiproof(self) -> DecodedMultiProof {
        match self {
            Self::AccountMultiproof { proof, stats: _ } => proof,
            Self::StorageProof { hashed_address, proof } => {
                DecodedMultiProof::from_storage_proof(hashed_address, proof)
            }
        }
    }
}
/// Channel used by worker threads to deliver `ProofResultMessage` items back to
/// `MultiProofTask`.
///
/// Workers use this sender to deliver proof results directly to `MultiProofTask`.
pub type ProofResultSender = CrossbeamSender<ProofResultMessage>;

/// Message containing a completed proof result with metadata for direct delivery to
/// `MultiProofTask`.
///
/// This type enables workers to send proof results directly to the `MultiProofTask` event loop.
#[derive(Debug)]
pub struct ProofResultMessage {
    /// Sequence number for ordering proofs
    pub sequence_number: u64,
    /// The proof calculation result (either account multiproof or storage proof)
    pub result: Result<ProofResult, ParallelStateRootError>,
    /// Time taken for the entire proof calculation (from dispatch to completion)
    pub elapsed: Duration,
    /// Original state update that triggered this proof
    pub state: HashedPostState,
}

/// Context for sending proof calculation results back to `MultiProofTask`.
///
/// This struct contains all context needed to send and track proof calculation results.
/// Workers use this to deliver completed proofs back to the main event loop.
#[derive(Debug, Clone)]
pub struct ProofResultContext {
    /// Channel sender for result delivery
    pub sender: ProofResultSender,
    /// Sequence number for proof ordering
    pub sequence_number: u64,
    /// Original state update that triggered this proof
    pub state: HashedPostState,
    /// Calculation start time for measuring elapsed duration
    pub start_time: Instant,
}

impl ProofResultContext {
    /// Creates a new proof result context.
    pub const fn new(
        sender: ProofResultSender,
        sequence_number: u64,
        state: HashedPostState,
        start_time: Instant,
    ) -> Self {
        Self { sender, sequence_number, state, start_time }
    }
}

/// Builds an account multiproof by consuming storage proof receivers lazily during trie walk.
///
/// This helper function enables interleaved parallelism: it builds the account subtree proof
/// while dynamically spawned storage proof tasks are still computing in parallel. Receivers
/// are consumed only when needed, allowing the account trie walk to proceed concurrently
/// with storage proof computation.
///
/// Returns a `DecodedMultiProof` containing the account subtree and storage proofs.
fn build_account_multiproof_with_storage_roots<P>(
    provider: &P,
    ctx: AccountMultiproofParams<'_>,
    tracker: &mut ParallelTrieTracker,
) -> Result<DecodedMultiProof, ParallelStateRootError>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    let accounts_added_removed_keys =
        ctx.multi_added_removed_keys.as_ref().map(|keys| keys.get_accounts());

    // Create the walker.
    let walker = TrieWalker::<_>::state_trie(
        provider.account_trie_cursor().map_err(ProviderError::Database)?,
        ctx.prefix_set,
    )
    .with_added_removed_keys(accounts_added_removed_keys)
    .with_deletions_retained(true);

    // Create a hash builder to rebuild the root node since it is not available in the database.
    let retainer = ctx
        .targets
        .keys()
        .map(Nibbles::unpack)
        .collect::<ProofRetainer>()
        .with_added_removed_keys(accounts_added_removed_keys);
    let mut hash_builder = HashBuilder::default()
        .with_proof_retainer(retainer)
        .with_updates(ctx.collect_branch_node_masks);

    // Initialize storage multiproofs map with pre-allocated capacity.
    // Proofs will be inserted as they're consumed from receivers during trie walk.
    let mut collected_decoded_storages: B256Map<DecodedStorageMultiProof> =
        B256Map::with_capacity_and_hasher(ctx.targets.len(), Default::default());
    let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
    let mut account_node_iter = TrieNodeIter::state_trie(
        walker,
        provider.hashed_account_cursor().map_err(ProviderError::Database)?,
    );

    let mut storage_proof_receivers = ctx.storage_proof_receivers;

    while let Some(account_node) = account_node_iter.try_next().map_err(ProviderError::Database)? {
        match account_node {
            TrieElement::Branch(node) => {
                hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
            }
            TrieElement::Leaf(hashed_address, account) => {
                let root = match storage_proof_receivers.remove(&hashed_address) {
                    Some(receiver) => {
                        // Block on this specific storage proof receiver - enables interleaved
                        // parallelism
                        let proof_msg = receiver.recv().map_err(|_| {
                            ParallelStateRootError::StorageRoot(
                                reth_execution_errors::StorageRootError::Database(
                                    DatabaseError::Other(format!(
                                        "Storage proof channel closed for {hashed_address}"
                                    )),
                                ),
                            )
                        })?;

                        // Extract storage proof from the result
                        let proof = match proof_msg.result? {
                            ProofResult::StorageProof { hashed_address: addr, proof } => {
                                debug_assert_eq!(
                                    addr,
                                    hashed_address,
                                    "storage worker must return same address: expected {hashed_address}, got {addr}"
                                );
                                proof
                            }
                            ProofResult::AccountMultiproof { .. } => {
                                unreachable!("storage worker only sends StorageProof variant")
                            }
                        };

                        let root = proof.root;
                        collected_decoded_storages.insert(hashed_address, proof);
                        root
                    }
                    // Since we do not store all intermediate nodes in the database, there might
                    // be a possibility of re-adding a non-modified leaf to the hash builder.
                    None => {
                        tracker.inc_missed_leaves();

                        match ctx.missed_leaves_storage_roots.entry(hashed_address) {
                            dashmap::Entry::Occupied(occ) => *occ.get(),
                            dashmap::Entry::Vacant(vac) => {
                                let root =
                                    StorageProof::new_hashed(provider, provider, hashed_address)
                                        .with_prefix_set_mut(Default::default())
                                        .storage_multiproof(
                                            ctx.targets
                                                .get(&hashed_address)
                                                .cloned()
                                                .unwrap_or_default(),
                                        )
                                        .map_err(|e| {
                                            ParallelStateRootError::StorageRoot(
                                                reth_execution_errors::StorageRootError::Database(
                                                    DatabaseError::Other(e.to_string()),
                                                ),
                                            )
                                        })?
                                        .root;

                                vac.insert(root);
                                root
                            }
                        }
                    }
                };

                // Encode account
                account_rlp.clear();
                let account = account.into_trie_account(root);
                account.encode(&mut account_rlp as &mut dyn BufMut);

                hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
            }
        }
    }

    // Consume remaining storage proof receivers for accounts not encountered during trie walk.
    for (hashed_address, receiver) in storage_proof_receivers {
        if let Ok(proof_msg) = receiver.recv() {
            // Extract storage proof from the result
            if let Ok(ProofResult::StorageProof { proof, .. }) = proof_msg.result {
                collected_decoded_storages.insert(hashed_address, proof);
            }
        }
    }

    let _ = hash_builder.root();

    let account_subtree_raw_nodes = hash_builder.take_proof_nodes();
    let decoded_account_subtree = DecodedProofNodes::try_from(account_subtree_raw_nodes)?;

    let (branch_node_hash_masks, branch_node_tree_masks) = if ctx.collect_branch_node_masks {
        let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
        (
            updated_branch_nodes.iter().map(|(path, node)| (*path, node.hash_mask)).collect(),
            updated_branch_nodes.into_iter().map(|(path, node)| (path, node.tree_mask)).collect(),
        )
    } else {
        (Default::default(), Default::default())
    };

    Ok(DecodedMultiProof {
        account_subtree: decoded_account_subtree,
        branch_node_hash_masks,
        branch_node_tree_masks,
        storages: collected_decoded_storages,
    })
}

/// Dispatches storage proofs as dynamic tasks and returns receivers.
///
/// Unlike the old channel-based dispatch, this spawns tasks on-demand
/// rather than queuing work to pre-spawned workers.
fn dispatch_storage_proofs_dynamic<Factory>(
    handle: &ProofWorkerHandle<Factory>,
    targets: &MultiProofTargets,
    storage_prefix_sets: &mut B256Map<PrefixSet>,
    with_branch_node_masks: bool,
    multi_added_removed_keys: Option<&Arc<MultiAddedRemovedKeys>>,
) -> Result<B256Map<CrossbeamReceiver<ProofResultMessage>>, ParallelStateRootError>
where
    Factory: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    let mut storage_proof_receivers =
        B256Map::with_capacity_and_hasher(targets.len(), Default::default());

    // Dispatch all storage proofs as dynamic tasks
    for (hashed_address, target_slots) in targets.iter() {
        let prefix_set = storage_prefix_sets.remove(hashed_address).unwrap_or_default();

        // Create channel for receiving ProofResultMessage
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let start = Instant::now();

        // Create computation input
        let input = StorageProofInput::new(
            *hashed_address,
            prefix_set,
            target_slots.clone(),
            with_branch_node_masks,
            multi_added_removed_keys.cloned(),
        );

        // Dispatch as dynamic task (not to a worker queue)
        handle
            .dispatch_storage_proof(
                input,
                ProofResultContext::new(result_tx, 0, HashedPostState::default(), start),
            )
            .map_err(|e| {
                ParallelStateRootError::Other(format!(
                    "Failed to dispatch storage proof for {}: {}",
                    hashed_address, e
                ))
            })?;

        storage_proof_receivers.insert(*hashed_address, result_rx);
    }

    Ok(storage_proof_receivers)
}
/// Input parameters for storage proof computation.
#[derive(Debug)]
pub struct StorageProofInput {
    /// The hashed address for which the proof is calculated.
    hashed_address: B256,
    /// The prefix set for the proof calculation.
    prefix_set: PrefixSet,
    /// The target slots for the proof calculation.
    target_slots: B256Set,
    /// Whether or not to collect branch node masks
    with_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
}

impl StorageProofInput {
    /// Creates a new [`StorageProofInput`] with the given hashed address, prefix set, and target
    /// slots.
    pub const fn new(
        hashed_address: B256,
        prefix_set: PrefixSet,
        target_slots: B256Set,
        with_branch_node_masks: bool,
        multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    ) -> Self {
        Self {
            hashed_address,
            prefix_set,
            target_slots,
            with_branch_node_masks,
            multi_added_removed_keys,
        }
    }
}
/// Input parameters for account multiproof computation.
#[derive(Debug, Clone)]
pub struct AccountMultiproofInput {
    /// The targets for which to compute the multiproof.
    pub targets: MultiProofTargets,
    /// The prefix sets for the proof calculation.
    pub prefix_sets: TriePrefixSets,
    /// Whether or not to collect branch node masks.
    pub collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    pub multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    pub missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
    /// Context for sending the proof result.
    pub proof_result_sender: ProofResultContext,
}

/// Parameters for building an account multiproof with pre-computed storage roots.
struct AccountMultiproofParams<'a> {
    /// The targets for which to compute the multiproof.
    targets: &'a MultiProofTargets,
    /// The prefix set for the account trie walk.
    prefix_set: PrefixSet,
    /// Whether or not to collect branch node masks.
    collect_branch_node_masks: bool,
    /// Provided by the user to give the necessary context to retain extra proofs.
    multi_added_removed_keys: Option<&'a Arc<MultiAddedRemovedKeys>>,
    /// Receivers for storage proofs being computed in parallel.
    storage_proof_receivers: B256Map<CrossbeamReceiver<ProofResultMessage>>,
    /// Cached storage proof roots for missed leaves encountered during account trie walk.
    missed_leaves_storage_roots: &'a DashMap<B256, B256>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_trie_common::prefix_set::TriePrefixSetsMut;
    use std::sync::Arc;
    use tokio::{runtime::Builder, task};

    fn test_ctx<Factory>(factory: Factory) -> ProofTaskCtx<Factory> {
        ProofTaskCtx::new(factory, Arc::new(TriePrefixSetsMut::default()))
    }

    /// Ensures `ProofWorkerHandle::new` spawns workers correctly.
    #[test]
    fn spawn_proof_workers_creates_handle() {
        let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        runtime.block_on(async {
            let handle = tokio::runtime::Handle::current();
            let provider_factory = create_test_provider_factory();
            let factory =
                reth_provider::providers::OverlayStateProviderFactory::new(provider_factory);
            let ctx = test_ctx(factory);

            let proof_handle = ProofWorkerHandle::new(handle.clone(), ctx, 5, 3);

            // Verify handle can be cloned
            let _cloned_handle = proof_handle.clone();

            // Workers shut down automatically when handle is dropped
            drop(proof_handle);
            task::yield_now().await;
        });
    }
}
