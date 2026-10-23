//! Parallel verification of persisted account and storage tries.

use self::{compare::SingleVerifier, pipeline::StoragePipeline};
use crate::{
    hashed_cursor::{HashedCursor, HashedStorageCursor},
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        TrieCursor, TrieStorageCursor,
    },
    Nibbles,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::BranchNodeCompact;
use reth_execution_errors::StateRootError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use std::{collections::VecDeque, thread};

mod compare;
mod pipeline;
mod storage;
pub use storage::StorageWorker;

/// Compares the persisted account and storage trie tables against tries recomputed from the hashed
/// state tables, yielding an [`Output`] for every inconsistency found.
///
/// The hashed tables are the source of truth. Storage tries are recomputed in parallel, so storage
/// findings are not ordered relative to account findings or to [`Output::Progress`]. Iteration
/// stops after the first error.
///
/// Memory use grows with the number of storage roots computed ahead of the account walk.
#[derive(Debug)]
pub struct Verifier<T: TrieCursor, H, S> {
    accounts: H,
    storage: S,
    account_verifier: SingleVerifier<DepthFirstTrieIterator<T>>,
    hash_builder: crate::HashBuilder,
    account_rlp: Vec<u8>,
    accounts_seeked: bool,
    pipeline: StoragePipeline,
    outputs: VecDeque<Result<Output, StateRootError>>,
    complete: bool,
}

const ACCOUNT_UPDATE_THRESHOLD: usize = 1024;

impl<T, H, S> Verifier<T, H, S>
where
    T: TrieCursor,
    H: HashedCursor<Value = Account>,
    S: HashedStorageCursor<Value = U256>,
{
    /// Starts verification and its background storage root computation.
    ///
    /// - `trie` reads the account trie table.
    /// - `accounts` and `lookahead` each read the hashed accounts table. `lookahead` is walked on a
    ///   background thread to schedule storage work ahead of `accounts`.
    /// - `storage` reads the hashed storage table.
    /// - `storage_workers` sets the storage parallelism, one thread per worker.
    ///
    /// Storage cursors may start at any hashed address. All cursors must read the same database
    /// version, and any transactions backing them must stay open until the verifier is dropped.
    /// Dropping the verifier cancels and joins background work.
    ///
    /// # Errors
    ///
    /// Returns an error if the account trie cannot be read or the background thread cannot be
    /// spawned.
    ///
    /// # Panics
    ///
    /// Panics if `storage_workers` is empty.
    pub fn new<L, W, C, TX>(
        trie: T,
        accounts: H,
        storage: S,
        lookahead: L,
        storage_workers: Vec<StorageWorker<W, C, TX>>,
    ) -> Result<Self, DatabaseError>
    where
        L: HashedCursor<Value = Account> + Send + 'static,
        W: TrieStorageCursor + Send + 'static,
        C: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        let account_verifier = SingleVerifier::new(None, trie)?;
        let pipeline = StoragePipeline::spawn(lookahead, storage_workers)?;
        Ok(Self {
            accounts,
            storage,
            account_verifier,
            hash_builder: crate::HashBuilder::default().with_updates(true),
            account_rlp: Vec::with_capacity(crate::TRIE_ACCOUNT_RLP_MAX_SIZE),
            accounts_seeked: false,
            pipeline,
            outputs: Default::default(),
            complete: false,
        })
    }

    fn fill_outputs(&mut self) -> Result<(), StateRootError> {
        loop {
            self.pipeline.drain(&mut self.outputs)?;
            if !self.outputs.is_empty() {
                return Ok(())
            }
            let next = if self.accounts_seeked {
                self.accounts.next()?
            } else {
                self.accounts_seeked = true;
                self.accounts.seek(B256::ZERO)?
            };
            let Some((address, account)) = next else { return self.finish() };
            // Storage workers make the same emptiness check and only send roots for non-empty
            // storage, so waiting on the pipeline here for an empty storage would never return.
            self.storage.set_hashed_address(address);
            let root = if self.storage.is_storage_empty()? {
                alloy_consensus::EMPTY_ROOT_HASH
            } else {
                self.pipeline.root(address, &mut self.outputs)?
            };
            self.account_rlp.clear();
            account.into_trie_account(root).encode(&mut self.account_rlp);
            self.hash_builder.add_leaf(Nibbles::unpack(address), &self.account_rlp);
            if self.hash_builder.updates_len() >= ACCOUNT_UPDATE_THRESHOLD {
                self.verify_account_updates()?;
            }
        }
    }

    fn finish(&mut self) -> Result<(), StateRootError> {
        self.pipeline.finish();
        self.complete = true;
        // Computing the root flushes the remaining branch node updates.
        self.hash_builder.root();
        self.verify_account_updates()?;
        let mut found = Vec::new();
        self.account_verifier.finalize(&mut found)?;
        self.pipeline.report(found, &mut self.outputs)
    }

    fn verify_account_updates(&mut self) -> Result<(), StateRootError> {
        let (builder, updates) = std::mem::take(&mut self.hash_builder).split();
        self.hash_builder = builder.with_updates(true);
        // The account trie root is calculated but never persisted. Updates are sorted depth-first
        // because that is the order in which the `HashBuilder` completes branch nodes, so each
        // batch continues where the previous one ended.
        let mut nodes =
            updates.into_iter().filter(|(path, _)| !path.is_empty()).collect::<Vec<_>>();
        nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
        let mut found = Vec::new();
        for (path, node) in nodes {
            self.account_verifier.next(&mut found, path, node)?;
            found.push(Output::Progress(path));
        }
        self.pipeline.report(found, &mut self.outputs)
    }
}

impl<T, H, S> Iterator for Verifier<T, H, S>
where
    T: TrieCursor,
    H: HashedCursor<Value = Account>,
    S: HashedStorageCursor<Value = U256>,
{
    type Item = Result<Output, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(output) = self.outputs.pop_front() {
                return Some(output)
            }
            if self.complete {
                return None
            }
            if let Err(error) = self.fill_outputs() {
                self.complete = true;
                self.pipeline.stop();
                self.outputs.push_back(Err(error));
            }
        }
    }
}

/// Output describes an inconsistency found when comparing the hashed state tables
/// ([`HashedCursor`]) with that of the trie tables ([`TrieCursor`]). The hashed
/// tables are considered the source of truth; outputs are on the part of the trie tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// An extra account node was found.
    AccountExtra(Nibbles, BranchNodeCompact),
    /// An extra storage node was found.
    StorageExtra(B256, Nibbles, BranchNodeCompact),
    /// An account node had the wrong value.
    AccountWrong {
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// A storage node had the wrong value.
    StorageWrong {
        /// The account the storage trie belongs to.
        account: B256,
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// An account node was missing.
    AccountMissing(Nibbles, BranchNodeCompact),
    /// A storage node was missing.
    StorageMissing(B256, Nibbles, BranchNodeCompact),
    /// Progress indicator with the path of the last verified account trie node.
    Progress(Nibbles),
}

/// Returns the default number of [`StorageWorker`]s to pass to [`Verifier::new`]: one per available
/// CPU, or one if the CPU count cannot be determined.
pub fn default_storage_workers() -> usize {
    thread::available_parallelism().map_or(1, usize::from)
}
