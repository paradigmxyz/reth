//! Opt-in `QMDb` state support for Reth SDK chains.
//!
//! Vanilla Ethereum Reth continues to use the Merkle Patricia Trie through the existing provider
//! stack. This crate gives custom chains a separate state-root provider that commits
//! [`HashedPostState`] overlays to Commonware `QMDb`.

use alloy_eips::{BlockNumHash, BlockNumberOrTag};
use alloy_genesis::Genesis;
use alloy_primitives::{
    keccak256, Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, B256, U256,
};
use commonware_cryptography::{sha256, Sha256};
use commonware_runtime::{buffer, tokio, BufferPooler, Runner as _, ThreadPooler};
use commonware_storage::{
    journal::contiguous::fixed::Config as JournalConfig,
    merkle::Location,
    mmr::{self, journaled::Config as MmrConfig},
    qmdb::{
        any::unordered::fixed::Update as Unordered,
        current::{batch::MerkleizedBatch, unordered::fixed::Db as UnorderedFixedDb, FixedConfig},
    },
    translator::EightCap,
};
use commonware_utils::{sequence::FixedBytes, NZUsize, NZU16, NZU64};
use futures_util::future::try_join_all;
use reth_chainspec::{ChainInfo, ChainSpecProvider};
use reth_primitives_traits::{Account, AlloyBlockHeader, Bytecode};
use reth_revm::db::BundleState;
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_storage_api::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BytecodeReader, ChangeSetReader,
    HashedPostStateProvider, HeaderProvider, StateProofProvider, StateProvider, StateProviderBox,
    StateProviderFactory, StateRootProvider, StorageChangeSetReader, StorageReader,
    StorageRootProvider,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie_common::{
    updates::TrieUpdates, AccountProof, DecodedMultiProofV2, ExecutionWitnessMode, HashedPostState,
    HashedStorage, MultiProof, MultiProofTargets, MultiProofTargetsV2, StorageMultiProof,
    StorageProof, TrieInput,
};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::Instant,
};
use tracing::debug;

const KEY_BYTES: usize = 65;
const VALUE_BYTES: usize = 74;
const CHUNK_SIZE: usize = 32;
const ACCOUNT_TAG: u8 = 0;
const STORAGE_TAG: u8 = 1;
const INITIAL_LOG_SIZE: u64 = 1;
const JOURNAL_MAGIC: &[u8; 8] = b"RQMDBJ1\0";
const JOURNAL_BODY_BYTES: usize = 120;
const JOURNAL_CHECKSUM_BYTES: usize = 32;
const JOURNAL_RECORD_BYTES: usize = JOURNAL_BODY_BYTES + JOURNAL_CHECKSUM_BYTES;
const QMDB_STAGE_BATCH_BLOCKS: u64 = 1_000;
// Commonware MMR operations can exceed the platform default thread stack on long chains.
const QMDB_ACTOR_STACK_SIZE: usize = 64 * 1024 * 1024;

pub type QmdbKey = FixedBytes<KEY_BYTES>;
pub type QmdbValue = FixedBytes<VALUE_BYTES>;
type QmdbLocation = Location<mmr::Family>;
type QmdbDb<E> = UnorderedFixedDb<mmr::Family, E, QmdbKey, QmdbValue, Sha256, EightCap, CHUNK_SIZE>;
type QmdbBatch =
    Arc<MerkleizedBatch<mmr::Family, sha256::Digest, Unordered<QmdbKey, QmdbValue>, CHUNK_SIZE>>;

/// Stage ID used by the staged pipeline integration.
pub const QMDB_STAGE_ID: StageId = StageId::Other("QmdbStateRoot");

/// Configuration for a QMDb-backed state store.
#[derive(Clone, Debug)]
pub struct QmdbConfig {
    /// Base directory used by the Commonware runtime for `QMDb` storage partitions.
    pub path: PathBuf,
    /// Prefix prepended to all `QMDb` partitions opened by this state store.
    pub partition_prefix: String,
    /// Worker threads used by the dedicated Commonware Tokio runtime.
    pub worker_threads: usize,
}

impl QmdbConfig {
    /// Creates a configuration rooted at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), partition_prefix: "reth-qmdb".to_string(), worker_threads: 2 }
    }

    /// Sets the partition prefix.
    pub fn with_partition_prefix(mut self, partition_prefix: impl Into<String>) -> Self {
        self.partition_prefix = partition_prefix.into();
        self
    }

    /// Sets the Commonware runtime worker thread count.
    pub const fn with_worker_threads(mut self, worker_threads: usize) -> Self {
        self.worker_threads = worker_threads;
        self
    }

    fn journal_path(&self) -> PathBuf {
        self.path.join(format!("{}-commit-journal.bin", self.partition_prefix))
    }
}

/// Result of evaluating or committing a `QMDb` state update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QmdbCommit {
    /// Canonical `QMDb` root after applying the state update.
    pub root: B256,
    /// Number of key/value mutations staged in `QMDb`.
    pub entries: usize,
}

/// A block descriptor used by [`QmdbChain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QmdbBlock {
    /// Block number.
    pub number: u64,
    /// Block hash.
    pub hash: B256,
    /// Parent block hash.
    pub parent_hash: B256,
}

/// Pre-encoded `QMDb` mutations for a block that has already had its root computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QmdbBlockMutations {
    /// Committed block.
    pub block: QmdbBlock,
    /// Expected `QMDb` root after applying `mutations` to the current durable head.
    pub expected_root: B256,
    /// Last-write-wins flat key/value mutations for this block.
    pub mutations: Vec<(QmdbKey, Option<QmdbValue>)>,
}

/// Canonical head tracked by [`QmdbChain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QmdbHead {
    /// Canonical head number.
    pub number: u64,
    /// Canonical head hash.
    pub hash: B256,
    /// `QMDb` root for the canonical head.
    pub root: B256,
}

/// Durable metadata for a block committed to `QMDb`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QmdbBlockCommit {
    /// Committed block.
    pub block: QmdbBlock,
    /// `QMDb` root after the block.
    pub root: B256,
    /// Number of committed `QMDb` log entries after the block.
    pub log_size: u64,
    /// Number of key/value mutations produced by the block.
    pub entries: u64,
}

impl QmdbBlockCommit {
    /// Returns the canonical head represented by this commit.
    pub const fn head(self) -> QmdbHead {
        QmdbHead { number: self.block.number, hash: self.block.hash, root: self.root }
    }
}

/// Errors returned by `QMDb` state operations.
#[derive(Debug, thiserror::Error)]
pub enum QmdbError {
    /// The dedicated `QMDb` actor is unavailable.
    #[error("QMDb actor is closed")]
    ActorClosed,
    /// The dedicated `QMDb` actor thread panicked.
    #[error("QMDb actor thread panicked")]
    ActorPanicked,
    /// The Commonware `QMDb` implementation returned an error.
    #[error("Commonware QMDb error: {0}")]
    Commonware(String),
    /// The canonical Reth provider returned an error while reconciling or replaying `QMDb` state.
    #[error("provider error: {0}")]
    Provider(#[source] ProviderError),
    /// `QMDb` returned malformed account bytes.
    #[error("invalid account value in QMDb")]
    InvalidAccountValue,
    /// `QMDb` returned malformed storage bytes.
    #[error("invalid storage value in QMDb")]
    InvalidStorageValue,
    /// `QMDb` returned a malformed storage key.
    #[error("invalid storage key in QMDb")]
    InvalidStorageKey,
    /// The canonical chain helper received a block that does not extend the current head.
    #[error("invalid parent hash: expected {expected}, got {got}")]
    InvalidParent {
        /// Expected parent hash.
        expected: B256,
        /// Actual parent hash.
        got: B256,
    },
    /// The requested block is not present in the durable `QMDb` commit journal.
    #[error("QMDb block {number} not found in commit journal")]
    UnknownBlock {
        /// Block number.
        number: u64,
    },
    /// A journaled block commit was attempted after low-level unanchored state writes.
    #[error("cannot journal block commits on top of unanchored QMDb state")]
    UnanchoredState,
    /// Low-level unanchored state writes were attempted after journaled block commits.
    #[error("cannot commit unanchored QMDb state after journaled block commits")]
    JournaledState,
    /// The durable commit journal could not be read or written.
    #[error("QMDb commit journal I/O error at {path}: {source}")]
    JournalIo {
        /// Journal path.
        path: PathBuf,
        /// Source I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The durable commit journal is malformed.
    #[error("invalid QMDb commit journal at {path}: {message}")]
    InvalidJournal {
        /// Journal path.
        path: PathBuf,
        /// Validation failure.
        message: &'static str,
    },
    /// The durable commit journal does not match the `QMDb` root at the same log size.
    #[error(
        "QMDb commit journal root mismatch at block {number}: expected {expected}, got {actual}"
    )]
    JournalRootMismatch {
        /// Block number.
        number: u64,
        /// Journal root.
        expected: B256,
        /// Current `QMDb` root.
        actual: B256,
    },
    /// The canonical database is ahead of `QMDb`; replay through the `QMDb` pipeline stage is
    /// required before the node can safely use `QMDb` roots.
    #[error("canonical database head {canonical_head} is ahead of QMDb head {qmdb_head}")]
    CanonicalAheadOfQmdb {
        /// Canonical database head.
        canonical_head: u64,
        /// Durable `QMDb` head.
        qmdb_head: u64,
    },
    /// The durable `QMDb` journal disagrees with the canonical hash at the same block.
    #[error("QMDb canonical hash mismatch at block {number}: expected {expected}, got {actual}")]
    CanonicalHashMismatch {
        /// Block number.
        number: u64,
        /// Canonical database hash.
        expected: B256,
        /// Durable `QMDb` journal hash.
        actual: B256,
    },
    /// The durable `QMDb` root disagrees with the canonical header state root.
    #[error("QMDb canonical root mismatch at block {number}: expected {expected}, got {actual}")]
    CanonicalRootMismatch {
        /// Block number.
        number: u64,
        /// Canonical header state root.
        expected: B256,
        /// Durable `QMDb` root.
        actual: B256,
    },
    /// A caller requested an MPT-specific proof or sub-root from a `QMDb` root provider.
    #[error("{0}")]
    MptUnsupported(&'static str),
    /// A mutex used by the `QMDb` chain helper was poisoned.
    #[error("QMDb chain lock poisoned")]
    LockPoisoned,
}

impl QmdbError {
    const fn provider(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

/// Persistent `QMDb` state actor.
#[derive(Clone, Debug)]
pub struct QmdbState {
    inner: Arc<QmdbActor>,
}

impl QmdbState {
    /// Opens a `QMDb` state store and starts its dedicated Commonware runtime thread.
    pub fn open(config: QmdbConfig) -> Result<Self, QmdbError> {
        let (command_tx, command_rx) = mpsc::channel();
        let (init_tx, init_rx) = mpsc::channel();
        let thread_name = format!("reth-qmdb-{}", config.partition_prefix);

        let handle = thread::Builder::new()
            .name(thread_name)
            .stack_size(QMDB_ACTOR_STACK_SIZE)
            .spawn(move || {
                let result = run_actor(config, command_rx, init_tx.clone());
                if let Err(err) = result {
                    let _ = init_tx.send(Err(err));
                }
            })
            .map_err(|_| QmdbError::ActorClosed)?;

        match init_rx.recv().map_err(|_| QmdbError::ActorClosed)? {
            Ok(()) => Ok(Self {
                inner: Arc::new(QmdbActor { command_tx, handle: Mutex::new(Some(handle)) }),
            }),
            Err(err) => {
                if handle.join().is_err() {
                    return Err(QmdbError::ActorPanicked);
                }
                Err(err)
            }
        }
    }

    /// Returns the currently committed `QMDb` root.
    pub fn root(&self) -> Result<B256, QmdbError> {
        self.request(Command::Root)
    }

    /// Computes the `QMDb` root for `hashed_state` without committing it.
    pub fn overlay_root(&self, hashed_state: HashedPostState) -> Result<QmdbCommit, QmdbError> {
        self.request(|response| Command::Overlay { hashed_state, response })
    }

    /// Computes a speculative root against an executed parent without changing durable state.
    pub fn preview(
        &self,
        parent_hash: B256,
        hashed_state: HashedPostState,
    ) -> Result<QmdbCommit, QmdbError> {
        self.request(|response| Command::Preview { parent_hash, hashed_state, response })
    }

    /// Associates a successfully executed block with its speculative root.
    pub fn remember(&self, block: QmdbBlock, root: B256) -> Result<(), QmdbError> {
        self.request(|response| Command::Remember { block, root, response })
    }

    /// Computes the `QMDb` root for already encoded, last-write-wins flat mutations without
    /// committing them.
    ///
    /// Callers must pass mutations filtered against the current durable `QMDb` state. Unlike
    /// [`Self::overlay_root`], this path does not perform per-key reads to remove no-op writes.
    pub fn overlay_mutations(
        &self,
        mutations: Vec<(QmdbKey, Option<QmdbValue>)>,
    ) -> Result<QmdbCommit, QmdbError> {
        self.request(|response| Command::OverlayMutations { mutations, response })
    }

    /// Commits `hashed_state` to `QMDb` and returns the resulting root.
    ///
    /// This is a low-level state-store helper. Chain integrations should use
    /// [`Self::commit_block`] or [`Self::commit_blocks`] so the root is tied to durable block
    /// metadata and can be unwound safely.
    pub fn commit_hashed_state(
        &self,
        hashed_state: HashedPostState,
    ) -> Result<QmdbCommit, QmdbError> {
        self.request(|response| Command::Commit { hashed_state, response })
    }

    /// Returns the durable canonical head tracked for journaled block commits.
    pub fn head(&self) -> Result<Option<QmdbHead>, QmdbError> {
        self.request(Command::Head)
    }

    /// Commits `hashed_state` for `block`, which must extend the durable canonical head.
    pub fn commit_block(
        &self,
        block: QmdbBlock,
        hashed_state: HashedPostState,
    ) -> Result<QmdbHead, QmdbError> {
        self.commit_blocks(vec![(block, hashed_state)])?
            .ok_or(QmdbError::UnknownBlock { number: block.number })
    }

    /// Commits a contiguous batch of blocks and syncs the `QMDb` log once for the whole batch.
    pub fn commit_blocks(
        &self,
        blocks: Vec<(QmdbBlock, HashedPostState)>,
    ) -> Result<Option<QmdbHead>, QmdbError> {
        self.request(|response| Command::CommitBlocks { blocks, response })
    }

    /// Commits already encoded block mutations and validates each resulting root.
    pub fn commit_block_mutations(
        &self,
        block: QmdbBlock,
        expected_root: B256,
        mutations: Vec<(QmdbKey, Option<QmdbValue>)>,
    ) -> Result<QmdbHead, QmdbError> {
        self.commit_blocks_mutations(vec![QmdbBlockMutations { block, expected_root, mutations }])?
            .ok_or(QmdbError::UnknownBlock { number: block.number })
    }

    /// Commits contiguous blocks from pre-encoded mutations and syncs the `QMDb` log once.
    pub fn commit_blocks_mutations(
        &self,
        blocks: Vec<QmdbBlockMutations>,
    ) -> Result<Option<QmdbHead>, QmdbError> {
        self.request(|response| Command::CommitBlocksMutations { blocks, response })
    }

    /// Rewinds `QMDb` and its durable block journal to `number`.
    ///
    /// Passing `0` keeps the journaled genesis commit if one exists. If no genesis commit exists,
    /// `QMDb` is rewound to its initial empty log and the block journal is cleared.
    pub fn rewind_to_block(&self, number: u64) -> Result<Option<QmdbHead>, QmdbError> {
        self.request(|response| Command::RewindToBlock { number, response })
    }

    /// Reconciles the durable `QMDb` head with the canonical database head.
    ///
    /// If `QMDb` is ahead, it is rewound to the canonical block. If the canonical database is
    /// ahead, this returns an error because missing `QMDb` roots must be reconstructed by replaying
    /// the staged `QMDb` integration, not silently skipped.
    pub fn reconcile_canonical<Provider>(
        &self,
        provider: &Provider,
    ) -> Result<Option<QmdbHead>, QmdbError>
    where
        Provider: BlockNumReader + HeaderProvider,
    {
        let canonical_head = provider.last_block_number().map_err(QmdbError::provider)?;
        let Some(mut qmdb_head) = self.head()? else {
            return if canonical_head == 0 {
                Ok(None)
            } else {
                Err(QmdbError::CanonicalAheadOfQmdb { canonical_head, qmdb_head: 0 })
            };
        };

        if qmdb_head.number > canonical_head {
            qmdb_head = match self.rewind_to_block(canonical_head)? {
                Some(head) => head,
                None => return Ok(None),
            };
        }

        if qmdb_head.number < canonical_head {
            return Err(QmdbError::CanonicalAheadOfQmdb {
                canonical_head,
                qmdb_head: qmdb_head.number,
            });
        }

        let expected_hash = provider
            .block_hash(qmdb_head.number)
            .map_err(QmdbError::provider)?
            .ok_or(QmdbError::UnknownBlock { number: qmdb_head.number })?;
        if expected_hash != qmdb_head.hash {
            return Err(QmdbError::CanonicalHashMismatch {
                number: qmdb_head.number,
                expected: expected_hash,
                actual: qmdb_head.hash,
            });
        }

        if qmdb_head.number != 0 {
            let expected_root = provider
                .header_by_number(qmdb_head.number)
                .map_err(QmdbError::provider)?
                .ok_or(QmdbError::UnknownBlock { number: qmdb_head.number })?
                .state_root();
            if expected_root != qmdb_head.root {
                return Err(QmdbError::CanonicalRootMismatch {
                    number: qmdb_head.number,
                    expected: expected_root,
                    actual: qmdb_head.root,
                });
            }
        }

        Ok(Some(qmdb_head))
    }

    /// Reads an account by hashed address.
    pub fn account(&self, hashed_address: B256) -> Result<Option<Account>, QmdbError> {
        self.request(|response| Command::Account { hashed_address, response })
    }

    /// Reads a storage slot by hashed address and hashed slot.
    pub fn storage(
        &self,
        hashed_address: B256,
        hashed_slot: B256,
    ) -> Result<Option<U256>, QmdbError> {
        self.request(|response| Command::Storage { hashed_address, hashed_slot, response })
    }

    /// Reads raw encoded `QMDb` values for `keys`.
    pub fn get_many(&self, keys: Vec<QmdbKey>) -> Result<Vec<Option<QmdbValue>>, QmdbError> {
        self.request(|response| Command::GetMany { keys, response })
    }

    fn request<T>(
        &self,
        command: impl FnOnce(mpsc::Sender<Result<T, QmdbError>>) -> Command,
    ) -> Result<T, QmdbError> {
        let (response_tx, response_rx) = mpsc::channel();
        self.inner.command_tx.send(command(response_tx)).map_err(|_| QmdbError::ActorClosed)?;
        response_rx.recv().map_err(|_| QmdbError::ActorClosed)?
    }
}

/// Builds the initial hashed post-state for a chain genesis allocation.
///
/// This anchors a `QMDb` journal at block zero so block one can extend the real canonical genesis
/// hash while subsequent headers use `QMDb` roots.
pub fn genesis_hashed_state(genesis: &Genesis) -> HashedPostState {
    let mut accounts = BTreeMap::new();
    let mut storages = BTreeMap::new();

    for (address, account) in &genesis.alloc {
        let hashed_address = keccak256(address);
        let bytecode_hash = account.code.as_ref().map(keccak256);
        accounts.insert(
            hashed_address,
            Some(Account {
                nonce: account.nonce.unwrap_or_default(),
                balance: account.balance,
                bytecode_hash,
            }),
        );

        if let Some(storage) = &account.storage {
            storages.insert(
                hashed_address,
                HashedStorage::from_iter(
                    storage
                        .iter()
                        .map(|(slot, value)| (keccak256(slot), U256::from_be_bytes(value.0))),
                ),
            );
        }
    }

    HashedPostState::default().with_accounts(accounts).with_storages(storages)
}

/// Staged-pipeline integration that commits executed block state into `QMDb` and validates headers
/// against the resulting `QMDb` root.
#[derive(Clone, Debug)]
pub struct QmdbStage {
    qmdb: QmdbState,
    batch_blocks: u64,
}

impl QmdbStage {
    /// Creates a `QMDb` stage with the default batch size.
    pub const fn new(qmdb: QmdbState) -> Self {
        Self { qmdb, batch_blocks: QMDB_STAGE_BATCH_BLOCKS }
    }

    /// Sets the maximum number of blocks committed to `QMDb` per stage execution.
    pub const fn with_batch_blocks(mut self, batch_blocks: u64) -> Self {
        self.batch_blocks = batch_blocks;
        self
    }

    /// Returns the underlying `QMDb` store.
    pub const fn qmdb(&self) -> &QmdbState {
        &self.qmdb
    }

    fn next_range(&self, input: ExecInput) -> Result<Option<(u64, u64)>, QmdbError> {
        let target = input.target();
        if target == 0 {
            return Ok(None);
        }

        let checkpoint = input.checkpoint().block_number;
        let mut next = checkpoint.saturating_add(1);

        match self.qmdb.head()? {
            Some(head) if head.number > checkpoint => {
                self.qmdb.rewind_to_block(checkpoint)?;
                next = checkpoint.saturating_add(1);
            }
            Some(head) if head.number < checkpoint => {
                next = head.number.saturating_add(1);
            }
            None if checkpoint > 0 => {
                next = 1;
            }
            Some(_) | None => {}
        }

        if next > target {
            return Ok(None);
        }

        let batch_blocks = self.batch_blocks.max(1);
        let end = next.saturating_add(batch_blocks - 1).min(target);
        Ok(Some((next, end)))
    }

    fn collect_blocks<Provider>(
        &self,
        provider: &Provider,
        from: u64,
        end: u64,
        target: u64,
    ) -> Result<Vec<(QmdbBlock, HashedPostState)>, QmdbError>
    where
        Provider: HeaderProvider
            + AccountReader
            + StorageReader
            + ChangeSetReader
            + StorageChangeSetReader,
    {
        let mut account_overrides = HashMap::new();
        let mut storage_overrides = HashMap::new();
        let mut blocks = Vec::with_capacity((end - from + 1) as usize);

        for number in (from..=target).rev() {
            let account_changes =
                provider.account_changesets_range(number..=number).map_err(QmdbError::provider)?;
            let storage_changes =
                provider.storage_changeset(number).map_err(QmdbError::provider)?;

            if number <= end {
                let header = provider
                    .sealed_header(number)
                    .map_err(QmdbError::provider)?
                    .ok_or(QmdbError::UnknownBlock { number })?;
                let mut accounts = BTreeMap::new();
                for (_, account_change) in &account_changes {
                    accounts.insert(
                        keccak256(account_change.address),
                        current_account(provider, &account_overrides, account_change.address)?,
                    );
                }

                let mut storages = BTreeMap::<_, HashedStorage>::new();
                for (block_address, storage_change) in &storage_changes {
                    let address = block_address.address();
                    let hashed_address = keccak256(address);
                    let storage = storages.entry(hashed_address).or_default();
                    let current =
                        current_storage(provider, &storage_overrides, address, storage_change.key)?;
                    storage.storage.insert(keccak256(storage_change.key), current);
                }

                blocks.push((
                    QmdbBlock { number, hash: header.hash(), parent_hash: header.parent_hash() },
                    HashedPostState::default().with_accounts(accounts).with_storages(storages),
                ));
            }

            for (_, account_change) in account_changes {
                account_overrides.insert(account_change.address, account_change.info);
            }
            for (block_address, storage_change) in storage_changes {
                storage_overrides
                    .insert((block_address.address(), storage_change.key), storage_change.value);
            }
        }

        blocks.reverse();
        Ok(blocks)
    }
}

impl<Provider> Stage<Provider> for QmdbStage
where
    Provider: HeaderProvider
        + AccountReader
        + StorageReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + Send,
{
    fn id(&self) -> StageId {
        QMDB_STAGE_ID
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        let Some((from, to)) = self.next_range(input).map_err(qmdb_stage_error)? else {
            return Ok(ExecOutput::done(input.checkpoint().with_block_number(input.target())));
        };

        let blocks =
            self.collect_blocks(provider, from, to, input.target()).map_err(qmdb_stage_error)?;

        let head = self.qmdb.commit_blocks(blocks).map_err(qmdb_stage_error)?;
        if let Some(head) = head {
            let expected = provider
                .header_by_number(head.number)
                .map_err(QmdbError::provider)
                .map_err(qmdb_stage_error)?
                .ok_or(QmdbError::UnknownBlock { number: head.number })
                .map_err(qmdb_stage_error)?
                .state_root();
            if head.root != expected {
                return Err(qmdb_stage_error(QmdbError::CanonicalRootMismatch {
                    number: head.number,
                    expected,
                    actual: head.root,
                }));
            }
        }

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(to), done: to == input.target() })
    }

    fn unwind(
        &mut self,
        _provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.qmdb.rewind_to_block(input.unwind_to).map_err(qmdb_stage_error)?;
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

fn qmdb_stage_error(error: QmdbError) -> StageError {
    StageError::Fatal(Box::new(error))
}

fn current_account<Provider: AccountReader>(
    provider: &Provider,
    overrides: &HashMap<Address, Option<Account>>,
    address: Address,
) -> Result<Option<Account>, QmdbError> {
    overrides
        .get(&address)
        .copied()
        .map(Ok)
        .unwrap_or_else(|| provider.basic_account(&address).map_err(QmdbError::provider))
}

fn current_storage<Provider: StorageReader>(
    provider: &Provider,
    overrides: &HashMap<(Address, B256), U256>,
    address: Address,
    key: B256,
) -> Result<U256, QmdbError> {
    overrides.get(&(address, key)).copied().map(Ok).unwrap_or_else(|| {
        provider
            .plain_state_storages([(address, [key])])
            .map(|mut values| {
                values
                    .pop()
                    .and_then(|(_, mut storage)| storage.pop())
                    .map(|entry| entry.value)
                    .unwrap_or_default()
            })
            .map_err(QmdbError::provider)
    })
}

/// A [`StateRootProvider`] adapter for custom chains that opt in to `QMDb`.
#[derive(Clone, Debug)]
pub struct QmdbStateRootProvider<S> {
    inner: S,
    qmdb: QmdbState,
}

impl<S> QmdbStateRootProvider<S> {
    /// Creates a new adapter.
    pub const fn new(inner: S, qmdb: QmdbState) -> Self {
        Self { inner, qmdb }
    }

    /// Returns the wrapped provider.
    pub const fn inner(&self) -> &S {
        &self.inner
    }

    /// Returns the `QMDb` state store.
    pub const fn qmdb(&self) -> &QmdbState {
        &self.qmdb
    }

    /// Consumes the adapter and returns the wrapped provider.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S> StateRootProvider for QmdbStateRootProvider<S> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.qmdb.overlay_root(hashed_state).map(|commit| commit.root).map_err(ProviderError::other)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.state_root(input.state)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_root(hashed_state).map(|root| (root, TrieUpdates::default()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_root_from_nodes(input).map(|root| (root, TrieUpdates::default()))
    }
}

impl<S: BlockHashReader> BlockHashReader for QmdbStateRootProvider<S> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.inner.canonical_hashes_range(start, end)
    }
}

impl<S: AccountReader> AccountReader for QmdbStateRootProvider<S> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.inner.basic_account(address)
    }
}

impl<S: BytecodeReader> BytecodeReader for QmdbStateRootProvider<S> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.inner.bytecode_by_hash(code_hash)
    }
}

impl<S: HashedPostStateProvider> HashedPostStateProvider for QmdbStateRootProvider<S> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> ProviderResult<HashedPostState> {
        self.inner.hashed_post_state(bundle_state)
    }
}

impl<S> StorageRootProvider for QmdbStateRootProvider<S> {
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        Err(qmdb_mpt_unsupported("QMDb storage roots are not MPT-compatible"))
    }

    fn storage_proof(
        &self,
        _address: Address,
        _slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        Err(qmdb_mpt_unsupported("QMDb storage proofs are not MPT-compatible"))
    }

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        Err(qmdb_mpt_unsupported("QMDb storage multiproofs are not MPT-compatible"))
    }
}

impl<S> StateProofProvider for QmdbStateRootProvider<S> {
    fn multiproof_v2(
        &self,
        _input: TrieInput,
        _targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        Err(qmdb_mpt_unsupported("QMDb state multiproofs are not MPT-compatible"))
    }
    fn proof(
        &self,
        _input: TrieInput,
        _address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Err(qmdb_mpt_unsupported("QMDb state proofs are not MPT-compatible"))
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        Err(qmdb_mpt_unsupported("QMDb state multiproofs are not MPT-compatible"))
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
        _mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        Err(qmdb_mpt_unsupported("QMDb execution witnesses are not MPT-compatible"))
    }
}

impl<S: StateProvider> StateProvider for QmdbStateRootProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        self.inner.storage(account, storage_key)
    }
}

/// A [`StateProviderFactory`] adapter that returns providers with `QMDb` state-root computation.
#[derive(Clone, Debug)]
pub struct QmdbStateProviderFactory<P> {
    inner: P,
    qmdb: QmdbState,
}

impl<P> QmdbStateProviderFactory<P> {
    /// Creates a new provider-factory adapter.
    pub const fn new(inner: P, qmdb: QmdbState) -> Self {
        Self { inner, qmdb }
    }

    /// Returns the wrapped provider factory.
    pub const fn inner(&self) -> &P {
        &self.inner
    }

    /// Returns the `QMDb` state store.
    pub const fn qmdb(&self) -> &QmdbState {
        &self.qmdb
    }

    /// Consumes the adapter and returns the wrapped provider factory.
    pub fn into_inner(self) -> P {
        self.inner
    }

    fn wrap(&self, provider: StateProviderBox) -> StateProviderBox {
        Box::new(QmdbStateRootProvider::new(provider, self.qmdb.clone()))
    }
}

impl<P: BlockHashReader> BlockHashReader for QmdbStateProviderFactory<P> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.inner.canonical_hashes_range(start, end)
    }
}

impl<P: BlockNumReader> BlockNumReader for QmdbStateProviderFactory<P> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        self.inner.chain_info()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        self.inner.best_block_number()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.inner.last_block_number()
    }

    fn earliest_block_number(&self) -> ProviderResult<BlockNumber> {
        self.inner.earliest_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.inner.block_number(hash)
    }
}

impl<P: BlockIdReader> BlockIdReader for QmdbStateProviderFactory<P> {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        self.inner.pending_block_num_hash()
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        self.inner.safe_block_num_hash()
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        self.inner.finalized_block_num_hash()
    }
}

impl<P: ChainSpecProvider> ChainSpecProvider for QmdbStateProviderFactory<P> {
    type ChainSpec = P::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.inner.chain_spec()
    }
}

impl<P: StateProviderFactory> StateProviderFactory for QmdbStateProviderFactory<P> {
    type Primitives = P::Primitives;

    fn state_with_block_appended(
        &self,
        parent_hash: BlockHash,
        block: reth_chain_state::ExecutedBlock<Self::Primitives>,
    ) -> ProviderResult<StateProviderBox> {
        self.inner.state_with_block_appended(parent_hash, block).map(|provider| self.wrap(provider))
    }
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        self.inner.latest().map(|provider| self.wrap(provider))
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        self.inner.state_by_block_number_or_tag(number_or_tag).map(|provider| self.wrap(provider))
    }

    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox> {
        self.inner.history_by_block_number(block).map(|provider| self.wrap(provider))
    }

    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        self.inner.history_by_block_hash(block).map(|provider| self.wrap(provider))
    }

    fn state_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        self.inner.state_by_block_hash(block).map(|provider| self.wrap(provider))
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        self.inner.pending().map(|provider| self.wrap(provider))
    }

    fn pending_state_by_hash(&self, block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        self.inner
            .pending_state_by_hash(block_hash)
            .map(|provider| provider.map(|provider| self.wrap(provider)))
    }

    fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>> {
        self.inner.maybe_pending().map(|provider| provider.map(|provider| self.wrap(provider)))
    }
}

/// Minimal canonical chain helper that commits block state to `QMDb` in order.
#[derive(Debug)]
pub struct QmdbChain {
    state: QmdbState,
}

impl QmdbChain {
    /// Creates a chain helper from a `QMDb` state store.
    pub const fn new(state: QmdbState) -> Self {
        Self { state }
    }

    /// Returns the underlying `QMDb` state store.
    pub const fn state(&self) -> &QmdbState {
        &self.state
    }

    /// Returns the current canonical head.
    pub fn head(&self) -> Result<Option<QmdbHead>, QmdbError> {
        self.state.head()
    }

    /// Commits `hashed_state` for `block`, which must extend the current head when one exists.
    pub fn commit_block(
        &self,
        block: QmdbBlock,
        hashed_state: HashedPostState,
    ) -> Result<QmdbHead, QmdbError> {
        self.state.commit_block(block, hashed_state)
    }

    /// Rewinds the canonical chain helper to `number`.
    pub fn rewind_to_block(&self, number: u64) -> Result<Option<QmdbHead>, QmdbError> {
        self.state.rewind_to_block(number)
    }
}

#[derive(Debug)]
struct QmdbActor {
    command_tx: mpsc::Sender<Command>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for QmdbActor {
    fn drop(&mut self) {
        let _ = self.command_tx.send(Command::Shutdown);
        let Ok(mut handle) = self.handle.lock() else {
            return;
        };
        if let Some(handle) = handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug)]
enum Command {
    Preview {
        parent_hash: B256,
        hashed_state: HashedPostState,
        response: mpsc::Sender<Result<QmdbCommit, QmdbError>>,
    },
    Remember {
        block: QmdbBlock,
        root: B256,
        response: mpsc::Sender<Result<(), QmdbError>>,
    },
    Root(mpsc::Sender<Result<B256, QmdbError>>),
    Head(mpsc::Sender<Result<Option<QmdbHead>, QmdbError>>),
    Overlay {
        hashed_state: HashedPostState,
        response: mpsc::Sender<Result<QmdbCommit, QmdbError>>,
    },
    Commit {
        hashed_state: HashedPostState,
        response: mpsc::Sender<Result<QmdbCommit, QmdbError>>,
    },
    CommitBlocks {
        blocks: Vec<(QmdbBlock, HashedPostState)>,
        response: mpsc::Sender<Result<Option<QmdbHead>, QmdbError>>,
    },
    CommitBlocksMutations {
        blocks: Vec<QmdbBlockMutations>,
        response: mpsc::Sender<Result<Option<QmdbHead>, QmdbError>>,
    },
    RewindToBlock {
        number: u64,
        response: mpsc::Sender<Result<Option<QmdbHead>, QmdbError>>,
    },
    Account {
        hashed_address: B256,
        response: mpsc::Sender<Result<Option<Account>, QmdbError>>,
    },
    Storage {
        hashed_address: B256,
        hashed_slot: B256,
        response: mpsc::Sender<Result<Option<U256>, QmdbError>>,
    },
    OverlayMutations {
        mutations: Vec<(QmdbKey, Option<QmdbValue>)>,
        response: mpsc::Sender<Result<QmdbCommit, QmdbError>>,
    },
    GetMany {
        keys: Vec<QmdbKey>,
        response: mpsc::Sender<Result<Vec<Option<QmdbValue>>, QmdbError>>,
    },
    Shutdown,
}

#[derive(Debug)]
struct QmdbCommitJournal {
    path: PathBuf,
    file: File,
    records: Vec<QmdbBlockCommit>,
}

impl QmdbCommitJournal {
    fn open(path: PathBuf) -> Result<Self, QmdbError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| QmdbError::JournalIo { path: parent.to_path_buf(), source })?;
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| QmdbError::JournalIo { path: path.clone(), source })?;

        let len = file
            .metadata()
            .map_err(|source| QmdbError::JournalIo { path: path.clone(), source })?
            .len();
        if len == 0 {
            file.write_all(JOURNAL_MAGIC)
                .and_then(|()| file.sync_all())
                .map_err(|source| QmdbError::JournalIo { path: path.clone(), source })?;
        }

        file.seek(SeekFrom::Start(0))
            .map_err(|source| QmdbError::JournalIo { path: path.clone(), source })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| QmdbError::JournalIo { path: path.clone(), source })?;

        if bytes.len() < JOURNAL_MAGIC.len() || &bytes[..JOURNAL_MAGIC.len()] != JOURNAL_MAGIC {
            return Err(QmdbError::InvalidJournal { path, message: "invalid journal header" });
        }

        let body = &bytes[JOURNAL_MAGIC.len()..];
        let complete = body.len() / JOURNAL_RECORD_BYTES;
        let trailing = body.len() % JOURNAL_RECORD_BYTES;
        let mut records = Vec::with_capacity(complete);
        for chunk in body[..complete * JOURNAL_RECORD_BYTES].chunks_exact(JOURNAL_RECORD_BYTES) {
            records.push(decode_journal_record(&path, chunk)?);
        }

        let mut journal = Self { path, file, records };
        if trailing != 0 {
            journal.truncate(complete)?;
        }
        journal.validate_chain()?;
        Ok(journal)
    }

    const fn len(&self) -> usize {
        self.records.len()
    }

    fn head(&self) -> Option<QmdbHead> {
        self.head_record().map(|commit| commit.head())
    }

    fn head_record(&self) -> Option<&QmdbBlockCommit> {
        self.records.last()
    }

    fn index_by_number(&self, number: u64) -> Option<usize> {
        self.records.binary_search_by_key(&number, |commit| commit.block.number).ok()
    }

    fn append(&mut self, records: &[QmdbBlockCommit]) -> Result<(), QmdbError> {
        if records.is_empty() {
            return Ok(());
        }

        self.file
            .seek(SeekFrom::End(0))
            .map_err(|source| QmdbError::JournalIo { path: self.path.clone(), source })?;
        for record in records {
            let encoded = encode_journal_record(record);
            self.file
                .write_all(&encoded)
                .map_err(|source| QmdbError::JournalIo { path: self.path.clone(), source })?;
        }
        self.file
            .sync_all()
            .map_err(|source| QmdbError::JournalIo { path: self.path.clone(), source })?;
        self.records.extend_from_slice(records);
        self.validate_chain()
    }

    fn truncate(&mut self, len: usize) -> Result<(), QmdbError> {
        self.records.truncate(len);
        let file_len = JOURNAL_MAGIC.len() + len * JOURNAL_RECORD_BYTES;
        self.file
            .set_len(file_len as u64)
            .and_then(|()| self.file.seek(SeekFrom::Start(file_len as u64)).map(|_| ()))
            .and_then(|()| self.file.sync_all())
            .map_err(|source| QmdbError::JournalIo { path: self.path.clone(), source })
    }

    fn validate_chain(&self) -> Result<(), QmdbError> {
        for window in self.records.windows(2) {
            let [parent, child] = window else {
                unreachable!("windows(2) returns exactly two elements");
            };
            if child.block.number != parent.block.number + 1 ||
                child.block.parent_hash != parent.block.hash
            {
                return Err(QmdbError::InvalidJournal {
                    path: self.path.clone(),
                    message: "non-contiguous block commits",
                });
            }
        }
        Ok(())
    }
}

fn encode_journal_record(record: &QmdbBlockCommit) -> [u8; JOURNAL_RECORD_BYTES] {
    let mut body = [0u8; JOURNAL_BODY_BYTES];
    body[0..8].copy_from_slice(&record.block.number.to_be_bytes());
    body[8..40].copy_from_slice(record.block.hash.as_slice());
    body[40..72].copy_from_slice(record.block.parent_hash.as_slice());
    body[72..104].copy_from_slice(record.root.as_slice());
    body[104..112].copy_from_slice(&record.log_size.to_be_bytes());
    body[112..120].copy_from_slice(&record.entries.to_be_bytes());

    let checksum = alloy_primitives::keccak256(body);
    let mut encoded = [0u8; JOURNAL_RECORD_BYTES];
    encoded[..JOURNAL_BODY_BYTES].copy_from_slice(&body);
    encoded[JOURNAL_BODY_BYTES..].copy_from_slice(checksum.as_slice());
    encoded
}

fn decode_journal_record(path: &Path, encoded: &[u8]) -> Result<QmdbBlockCommit, QmdbError> {
    if encoded.len() != JOURNAL_RECORD_BYTES {
        return Err(QmdbError::InvalidJournal {
            path: path.to_path_buf(),
            message: "invalid record length",
        });
    }

    let (body, checksum) = encoded.split_at(JOURNAL_BODY_BYTES);
    let expected = alloy_primitives::keccak256(body);
    if checksum != expected.as_slice() {
        return Err(QmdbError::InvalidJournal {
            path: path.to_path_buf(),
            message: "record checksum mismatch",
        });
    }

    let number = u64::from_be_bytes(body[0..8].try_into().expect("journal number is fixed size"));
    let hash = B256::from_slice(&body[8..40]);
    let parent_hash = B256::from_slice(&body[40..72]);
    let root = B256::from_slice(&body[72..104]);
    let log_size =
        u64::from_be_bytes(body[104..112].try_into().expect("journal log size is fixed size"));
    let entries =
        u64::from_be_bytes(body[112..120].try_into().expect("journal entries is fixed size"));

    Ok(QmdbBlockCommit { block: QmdbBlock { number, hash, parent_hash }, root, log_size, entries })
}

fn run_actor(
    config: QmdbConfig,
    command_rx: mpsc::Receiver<Command>,
    init_tx: mpsc::Sender<Result<(), QmdbError>>,
) -> Result<(), QmdbError> {
    let runtime_config = tokio::Config::new()
        .with_storage_directory(config.path.clone())
        .with_worker_threads(config.worker_threads.max(1));
    tokio::Runner::new(runtime_config).start(|context| async move {
        let qmdb_config = create_commonware_config(&context, &config)?;
        let mut db = QmdbDb::init(context, qmdb_config).await.map_err(commonware_error)?;
        let mut journal = QmdbCommitJournal::open(config.journal_path())?;
        let mut speculative = SpeculativeState::default();
        reconcile_journal(&mut db, &mut journal).await?;
        let _ = init_tx.send(Ok(()));

        while let Ok(command) = command_rx.recv() {
            if !handle_actor_command(command, &mut db, &mut journal, &mut speculative).await? {
                break;
            }
        }

        Ok(())
    })
}

async fn handle_actor_command<E>(
    command: Command,
    db: &mut QmdbDb<E>,
    journal: &mut QmdbCommitJournal,
    speculative: &mut SpeculativeState,
) -> Result<bool, QmdbError>
where
    E: commonware_storage::Context,
{
    match command {
        Command::Preview { parent_hash, hashed_state, response } => {
            let result = preview_root(db, journal, speculative, parent_hash, hashed_state).await;
            let _ = response.send(result);
        }
        Command::Remember { block, root, response } => {
            let result = if speculative.roots.contains_key(&root) {
                speculative.blocks.insert(block.hash, (block, root));
                Ok(())
            } else {
                Err(QmdbError::UnknownBlock { number: block.number })
            };
            let _ = response.send(result);
        }
        Command::Root(response) => {
            let _ = response.send(Ok(digest_to_b256(db.root())));
        }
        Command::Head(response) => {
            let _ = response.send(Ok(journal.head()));
        }
        Command::Overlay { hashed_state, response } => {
            let _ = response.send(update_root(db, hashed_state, UpdateMode::Overlay).await);
        }
        Command::Commit { hashed_state, response } => {
            let result = if journal.head_record().is_some() {
                Err(QmdbError::JournaledState)
            } else {
                update_root(db, hashed_state, UpdateMode::ApplyAndSync).await
            };
            let _ = response.send(result);
        }
        Command::CommitBlocks { blocks, response } => {
            let _ = response.send(commit_blocks(db, journal, blocks).await);
        }
        Command::CommitBlocksMutations { blocks, response } => {
            let _ = response.send(commit_blocks_mutations(db, journal, blocks).await);
        }
        Command::RewindToBlock { number, response } => {
            speculative.roots.clear();
            speculative.blocks.clear();
            let _ = response.send(rewind_to_block(db, journal, number).await);
        }
        Command::Account { hashed_address, response } => {
            let value = db
                .get(&account_key(&hashed_address))
                .await
                .map_err(commonware_error)
                .and_then(|value| value.map(decode_account).transpose());
            let _ = response.send(value);
        }
        Command::Storage { hashed_address, hashed_slot, response } => {
            let value = db
                .get(&storage_key(&hashed_address, &hashed_slot))
                .await
                .map_err(commonware_error)
                .and_then(|value| value.map(decode_storage).transpose());
            let _ = response.send(value);
        }
        Command::OverlayMutations { mutations, response } => {
            let _ = response.send(update_mutations_root(db, mutations, UpdateMode::Overlay).await);
        }
        Command::GetMany { keys, response } => {
            let values =
                try_join_all(keys.iter().map(|key| db.get(key))).await.map_err(commonware_error);
            let _ = response.send(values);
        }
        Command::Shutdown => return Ok(false),
    }
    Ok(true)
}

#[derive(Default)]
struct SpeculativeState {
    roots: HashMap<B256, QmdbBatch>,
    blocks: HashMap<B256, (QmdbBlock, B256)>,
}

async fn preview_root<E: commonware_storage::Context>(
    db: &QmdbDb<E>,
    journal: &QmdbCommitJournal,
    speculative: &mut SpeculativeState,
    parent_hash: B256,
    hashed_state: HashedPostState,
) -> Result<QmdbCommit, QmdbError> {
    let head = journal.head().ok_or(QmdbError::UnanchoredState)?;
    let parent = if head.hash == parent_hash {
        db.to_batch()
    } else {
        let mut cursor = parent_hash;
        let mut remaining = speculative.blocks.len();
        while cursor != head.hash {
            let (block, _) = speculative
                .blocks
                .get(&cursor)
                .ok_or(QmdbError::InvalidParent { expected: head.hash, got: parent_hash })?;
            if remaining == 0 {
                return Err(QmdbError::InvalidParent { expected: head.hash, got: parent_hash });
            }
            remaining -= 1;
            cursor = block.parent_hash;
        }
        let (_, root) =
            speculative.blocks.get(&parent_hash).expect("the parent ancestry was checked");
        speculative
            .roots
            .get(root)
            .cloned()
            .ok_or(QmdbError::InvalidParent { expected: head.hash, got: parent_hash })?
    };
    let mut desired = Vec::new();
    for (address, account) in &hashed_state.accounts {
        desired.push((account_key(address), account.as_ref().map(encode_account)));
    }
    for (address, storage) in &hashed_state.storages {
        for (slot, value) in &storage.storage {
            desired.push((
                storage_key(address, slot),
                (!value.is_zero()).then(|| encode_storage(*value)),
            ));
        }
    }
    desired.sort_unstable_by_key(|(key, _)| key.clone());
    let mut batch = parent.new_batch::<Sha256>();
    let mut entries = 0;
    for (key, value) in desired {
        if parent.get(&key, db).await.map_err(commonware_error)? != value {
            batch = batch.write(key, value);
            entries += 1;
        }
    }
    let merkleized = if entries == 0 {
        parent
    } else {
        batch.merkleize(db, None).await.map_err(commonware_error)?
    };
    let root = digest_to_b256(merkleized.root());
    speculative.roots.insert(root, merkleized);
    Ok(QmdbCommit { root, entries })
}

fn create_commonware_config(
    context: &(impl BufferPooler + ThreadPooler),
    config: &QmdbConfig,
) -> Result<FixedConfig<EightCap>, QmdbError> {
    let partition_prefix = &config.partition_prefix;
    let page_cache = buffer::paged::CacheRef::from_pooler(context, NZU16!(2048), NZUsize!(10));
    let thread_pool = context
        .create_thread_pool(
            NonZeroUsize::new(config.worker_threads.max(1))
                .expect("worker thread count is forced non-zero"),
        )
        .map_err(|error| {
            QmdbError::Commonware(format!("failed to create QMDB merkle thread pool: {error}"))
        })?;

    Ok(FixedConfig {
        merkle_config: MmrConfig {
            journal_partition: format!("{partition_prefix}-mmr-journal"),
            metadata_partition: format!("{partition_prefix}-mmr-metadata"),
            items_per_blob: NZU64!(4096),
            write_buffer: NZUsize!(4096),
            thread_pool: Some(thread_pool),
            page_cache: page_cache.clone(),
        },
        journal_config: JournalConfig {
            partition: format!("{partition_prefix}-log-journal"),
            items_per_blob: NZU64!(4096),
            write_buffer: NZUsize!(4096),
            page_cache,
        },
        grafted_metadata_partition: format!("{partition_prefix}-grafted-mmr-metadata"),
        translator: EightCap,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateMode {
    Overlay,
    Apply,
    ApplyAndSync,
}

async fn update_root<E>(
    db: &mut QmdbDb<E>,
    hashed_state: HashedPostState,
    mode: UpdateMode,
) -> Result<QmdbCommit, QmdbError>
where
    E: commonware_storage::Context,
{
    let started = Instant::now();
    let collect_started = Instant::now();
    let mutations = collect_mutations(db, &hashed_state).await?;
    let collect_duration = collect_started.elapsed();
    if mutations.is_empty() {
        debug!(
            target: "reth_qmdb",
            ?mode,
            collect_ms = collect_duration.as_secs_f64() * 1000.0,
            total_ms = started.elapsed().as_secs_f64() * 1000.0,
            "QMDB root update had no state mutations"
        );
        return Ok(QmdbCommit { root: digest_to_b256(db.root()), entries: 0 });
    }

    let commit = update_mutations_root(db, mutations, mode).await?;
    debug!(
        target: "reth_qmdb",
        ?mode,
        entries = commit.entries,
        collect_ms = collect_duration.as_secs_f64() * 1000.0,
        total_ms = started.elapsed().as_secs_f64() * 1000.0,
        "computed QMDB hashed-state root"
    );
    Ok(commit)
}

async fn update_mutations_root<E>(
    db: &mut QmdbDb<E>,
    mutations: Vec<(QmdbKey, Option<QmdbValue>)>,
    mode: UpdateMode,
) -> Result<QmdbCommit, QmdbError>
where
    E: commonware_storage::Context,
{
    let started = Instant::now();
    if mutations.is_empty() {
        debug!(
            target: "reth_qmdb",
            ?mode,
            total_ms = started.elapsed().as_secs_f64() * 1000.0,
            "QMDB flat-mutation root had no state mutations"
        );
        return Ok(QmdbCommit { root: digest_to_b256(db.root()), entries: 0 });
    }

    let entries = mutations.len();
    let build_started = Instant::now();
    let mut batch = db.new_batch();
    for (key, value) in mutations {
        batch = batch.write(key, value);
    }
    let build_duration = build_started.elapsed();

    let merkleize_started = Instant::now();
    let merkleized = batch.merkleize(db, None).await.map_err(commonware_error)?;
    let merkleize_duration = merkleize_started.elapsed();
    let root = digest_to_b256(merkleized.root());
    let apply_started = Instant::now();
    if mode != UpdateMode::Overlay {
        db.apply_batch(merkleized).await.map_err(commonware_error)?;
    }
    let apply_duration = apply_started.elapsed();
    let sync_started = Instant::now();
    if mode == UpdateMode::ApplyAndSync {
        db.sync().await.map_err(commonware_error)?;
    }
    let sync_duration = sync_started.elapsed();
    debug!(
        target: "reth_qmdb",
        ?mode,
        entries,
        build_ms = build_duration.as_secs_f64() * 1000.0,
        merkleize_ms = merkleize_duration.as_secs_f64() * 1000.0,
        apply_ms = apply_duration.as_secs_f64() * 1000.0,
        sync_ms = sync_duration.as_secs_f64() * 1000.0,
        total_ms = started.elapsed().as_secs_f64() * 1000.0,
        "computed QMDB flat-mutation root"
    );
    Ok(QmdbCommit { root, entries })
}

async fn commit_blocks<E>(
    db: &mut QmdbDb<E>,
    journal: &mut QmdbCommitJournal,
    blocks: Vec<(QmdbBlock, HashedPostState)>,
) -> Result<Option<QmdbHead>, QmdbError>
where
    E: commonware_storage::Context,
{
    if blocks.is_empty() {
        return Ok(journal.head());
    }

    let current_size = qmdb_log_size(db).await;
    if journal.head_record().is_none() && current_size > INITIAL_LOG_SIZE {
        return Err(QmdbError::UnanchoredState);
    }

    let rollback_size = journal.head_record().map_or(INITIAL_LOG_SIZE, |commit| commit.log_size);
    let mut parent = journal.head();
    let mut pending = Vec::with_capacity(blocks.len());

    for (block, hashed_state) in blocks {
        if let Some(current) = parent {
            if block.number != current.number + 1 || block.parent_hash != current.hash {
                return Err(QmdbError::InvalidParent {
                    expected: current.hash,
                    got: block.parent_hash,
                });
            }
        } else if block.number != 0 {
            return Err(QmdbError::UnknownBlock { number: block.number.saturating_sub(1) });
        }

        let commit = update_root(db, hashed_state, UpdateMode::Apply).await?;
        let log_size = qmdb_log_size(db).await;
        let block_commit =
            QmdbBlockCommit { block, root: commit.root, log_size, entries: commit.entries as u64 };
        parent = Some(block_commit.head());
        pending.push(block_commit);
    }

    db.sync().await.map_err(commonware_error)?;
    if let Err(err) = journal.append(&pending) {
        rewind_qmdb(db, rollback_size).await?;
        return Err(err);
    }

    Ok(journal.head())
}

async fn commit_blocks_mutations<E>(
    db: &mut QmdbDb<E>,
    journal: &mut QmdbCommitJournal,
    blocks: Vec<QmdbBlockMutations>,
) -> Result<Option<QmdbHead>, QmdbError>
where
    E: commonware_storage::Context,
{
    if blocks.is_empty() {
        return Ok(journal.head());
    }

    let current_size = qmdb_log_size(db).await;
    if journal.head_record().is_none() && current_size > INITIAL_LOG_SIZE {
        return Err(QmdbError::UnanchoredState);
    }

    let rollback_size = journal.head_record().map_or(INITIAL_LOG_SIZE, |commit| commit.log_size);
    let mut parent = journal.head();
    let mut pending = Vec::with_capacity(blocks.len());

    for QmdbBlockMutations { block, expected_root, mutations } in blocks {
        if let Some(current) = parent {
            if block.number != current.number + 1 || block.parent_hash != current.hash {
                return Err(QmdbError::InvalidParent {
                    expected: current.hash,
                    got: block.parent_hash,
                });
            }
        } else if block.number != 0 {
            return Err(QmdbError::UnknownBlock { number: block.number.saturating_sub(1) });
        }

        let commit = update_mutations_root(db, mutations, UpdateMode::Apply).await?;
        if commit.root != expected_root {
            rewind_qmdb(db, rollback_size).await?;
            return Err(QmdbError::CanonicalRootMismatch {
                number: block.number,
                expected: expected_root,
                actual: commit.root,
            });
        }

        let log_size = qmdb_log_size(db).await;
        let block_commit =
            QmdbBlockCommit { block, root: commit.root, log_size, entries: commit.entries as u64 };
        parent = Some(block_commit.head());
        pending.push(block_commit);
    }

    if let Err(err) = db.sync().await.map_err(commonware_error) {
        rewind_qmdb(db, rollback_size).await?;
        return Err(err);
    }
    if let Err(err) = journal.append(&pending) {
        rewind_qmdb(db, rollback_size).await?;
        return Err(err);
    }

    Ok(journal.head())
}

async fn rewind_to_block<E>(
    db: &mut QmdbDb<E>,
    journal: &mut QmdbCommitJournal,
    number: u64,
) -> Result<Option<QmdbHead>, QmdbError>
where
    E: commonware_storage::Context,
{
    if number == 0 && journal.index_by_number(0).is_none() {
        rewind_qmdb(db, INITIAL_LOG_SIZE).await?;
        journal.truncate(0)?;
        return Ok(None);
    }

    let Some(index) = journal.index_by_number(number) else {
        return Err(QmdbError::UnknownBlock { number });
    };
    let commit = journal.records[index];

    rewind_qmdb(db, commit.log_size).await?;
    if digest_to_b256(db.root()) != commit.root {
        return Err(QmdbError::JournalRootMismatch {
            number: commit.block.number,
            expected: commit.root,
            actual: digest_to_b256(db.root()),
        });
    }
    journal.truncate(index + 1)?;
    Ok(journal.head())
}

async fn reconcile_journal<E>(
    db: &mut QmdbDb<E>,
    journal: &mut QmdbCommitJournal,
) -> Result<(), QmdbError>
where
    E: commonware_storage::Context,
{
    let mut db_size = qmdb_log_size(db).await;

    while journal.head_record().is_some_and(|head| head.log_size > db_size) {
        journal.truncate(journal.len() - 1)?;
    }

    match journal.head_record().copied() {
        Some(head) if head.log_size == db_size && head.root == digest_to_b256(db.root()) => Ok(()),
        Some(head) if head.log_size <= db_size => {
            rewind_qmdb(db, head.log_size).await?;
            let actual = digest_to_b256(db.root());
            if actual != head.root {
                return Err(QmdbError::JournalRootMismatch {
                    number: head.block.number,
                    expected: head.root,
                    actual,
                });
            }
            Ok(())
        }
        Some(head) => Err(QmdbError::JournalRootMismatch {
            number: head.block.number,
            expected: head.root,
            actual: digest_to_b256(db.root()),
        }),
        None => {
            if db_size > INITIAL_LOG_SIZE {
                rewind_qmdb(db, INITIAL_LOG_SIZE).await?;
                db_size = qmdb_log_size(db).await;
            }
            if db_size != INITIAL_LOG_SIZE {
                return Err(QmdbError::InvalidJournal {
                    path: journal.path.clone(),
                    message: "empty journal cannot be reconciled with QMDb log size",
                });
            }
            Ok(())
        }
    }
}

async fn rewind_qmdb<E>(db: &mut QmdbDb<E>, log_size: u64) -> Result<(), QmdbError>
where
    E: commonware_storage::Context,
{
    db.rewind(QmdbLocation::new(log_size)).await.map_err(commonware_error)?;
    db.sync().await.map_err(commonware_error)
}

async fn qmdb_log_size<E>(db: &QmdbDb<E>) -> u64
where
    E: commonware_storage::Context,
{
    *db.bounds().await.end
}

async fn collect_mutations<E>(
    db: &QmdbDb<E>,
    hashed_state: &HashedPostState,
) -> Result<Vec<(QmdbKey, Option<QmdbValue>)>, QmdbError>
where
    E: commonware_storage::Context,
{
    let mut desired = Vec::new();
    let mut accounts: Vec<_> = hashed_state.accounts.iter().collect();
    accounts.sort_unstable_by_key(|(hashed_address, _)| **hashed_address);

    for (hashed_address, account) in accounts {
        let key = account_key(hashed_address);
        match account {
            Some(account) => {
                desired.push((key, Some(encode_account(account))));
            }
            None => {
                desired.push((key, None));
            }
        }
    }

    let mut storage_addresses: Vec<_> = hashed_state.storages.keys().copied().collect();
    storage_addresses.sort_unstable();

    for hashed_address in storage_addresses {
        let storage = hashed_state
            .storages
            .get(&hashed_address)
            .expect("storage address came from hashed state");
        for (slot, value) in storage_slots(&storage.storage) {
            let key = storage_key(&hashed_address, &slot);
            let value = (!value.is_zero()).then(|| encode_storage(value));
            desired.push((key, value));
        }
    }

    filter_noop_mutations(db, desired).await
}

async fn filter_noop_mutations<E>(
    db: &QmdbDb<E>,
    desired: Vec<(QmdbKey, Option<QmdbValue>)>,
) -> Result<Vec<(QmdbKey, Option<QmdbValue>)>, QmdbError>
where
    E: commonware_storage::Context,
{
    let existing =
        try_join_all(desired.iter().map(|(key, _)| db.get(key))).await.map_err(commonware_error)?;
    Ok(desired
        .into_iter()
        .zip(existing)
        .filter_map(|((key, value), existing)| (existing != value).then_some((key, value)))
        .collect())
}

fn storage_slots(storage: &alloy_primitives::map::B256Map<U256>) -> BTreeMap<B256, U256> {
    storage.iter().map(|(slot, value)| (*slot, *value)).collect()
}

fn account_key(hashed_address: &B256) -> QmdbKey {
    let mut key = [0; KEY_BYTES];
    key[0] = ACCOUNT_TAG;
    key[1..33].copy_from_slice(hashed_address.as_slice());
    QmdbKey::new(key)
}

fn storage_key(hashed_address: &B256, hashed_slot: &B256) -> QmdbKey {
    let mut key = [0; KEY_BYTES];
    key[0] = STORAGE_TAG;
    key[1..33].copy_from_slice(hashed_address.as_slice());
    key[33..65].copy_from_slice(hashed_slot.as_slice());
    QmdbKey::new(key)
}

fn encode_account(account: &Account) -> QmdbValue {
    let mut value = [0; VALUE_BYTES];
    value[0] = ACCOUNT_TAG;
    value[1..9].copy_from_slice(&account.nonce.to_be_bytes());
    value[9..41].copy_from_slice(&account.balance.to_be_bytes::<32>());
    if let Some(bytecode_hash) = account.bytecode_hash {
        value[41] = 1;
        value[42..74].copy_from_slice(bytecode_hash.as_slice());
    }
    QmdbValue::new(value)
}

fn decode_account(value: QmdbValue) -> Result<Account, QmdbError> {
    let bytes = value.as_ref();
    if bytes[0] != ACCOUNT_TAG || bytes[41] > 1 {
        return Err(QmdbError::InvalidAccountValue);
    }

    let nonce = u64::from_be_bytes(bytes[1..9].try_into().expect("account nonce has fixed size"));
    let balance_bytes: [u8; 32] = bytes[9..41].try_into().expect("account balance has fixed size");
    let balance = U256::from_be_bytes(balance_bytes);
    let bytecode_hash = (bytes[41] == 1).then(|| B256::from_slice(&bytes[42..74]));

    Ok(Account { nonce, balance, bytecode_hash })
}

fn encode_storage(storage: U256) -> QmdbValue {
    let mut value = [0; VALUE_BYTES];
    value[0] = STORAGE_TAG;
    value[1..33].copy_from_slice(&storage.to_be_bytes::<32>());
    QmdbValue::new(value)
}

fn decode_storage(value: QmdbValue) -> Result<U256, QmdbError> {
    let bytes = value.as_ref();
    if bytes[0] != STORAGE_TAG {
        return Err(QmdbError::InvalidStorageValue);
    }
    let storage_bytes: [u8; 32] = bytes[1..33].try_into().expect("storage value has fixed size");
    Ok(U256::from_be_bytes(storage_bytes))
}

fn digest_to_b256(digest: sha256::Digest) -> B256 {
    B256::from(digest.0)
}

fn commonware_error(error: commonware_storage::qmdb::Error<mmr::Family>) -> QmdbError {
    QmdbError::Commonware(error.to_string())
}

fn qmdb_mpt_unsupported(message: &'static str) -> ProviderError {
    ProviderError::other(QmdbError::MptUnsupported(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Bytes};
    use alloy_rlp::Decodable;
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::{AlloyBlockHeader, Block as _};
    use reth_trie_common::HashedStorage;
    use std::str::FromStr;

    #[test]
    fn commits_hashed_state_and_treats_wipes_as_sparse_updates() {
        let tempdir = tempfile::tempdir().unwrap();
        let qmdb = QmdbState::open(QmdbConfig::new(tempdir.path())).unwrap();
        let empty_root = qmdb.root().unwrap();
        let hashed_address = B256::with_last_byte(1);
        let slot_a = B256::with_last_byte(2);
        let slot_b = B256::with_last_byte(3);

        let account = Account { nonce: 7, balance: U256::from(11), bytecode_hash: None };
        let state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(account))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter([(slot_a, U256::from(13))]),
            )]);

        let first = qmdb.commit_hashed_state(state.clone()).unwrap();
        assert_ne!(first.root, empty_root);
        assert_eq!(first.entries, 2);
        assert_eq!(qmdb.account(hashed_address).unwrap(), Some(account));
        assert_eq!(qmdb.storage(hashed_address, slot_a).unwrap(), Some(U256::from(13)));

        let duplicate = qmdb.commit_hashed_state(state).unwrap();
        assert_eq!(duplicate.root, first.root);
        assert_eq!(duplicate.entries, 0);

        let wiped = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter([(slot_b, U256::from(17))]),
        )]);
        let second = qmdb.commit_hashed_state(wiped).unwrap();
        assert_ne!(second.root, first.root);
        assert_eq!(qmdb.storage(hashed_address, slot_a).unwrap(), Some(U256::from(13)));
        assert_eq!(qmdb.storage(hashed_address, slot_b).unwrap(), Some(U256::from(17)));

        let wiped_to_empty = HashedPostState::default()
            .with_storages([(hashed_address, HashedStorage::from_iter([(slot_b, U256::ZERO)]))]);
        let third = qmdb.commit_hashed_state(wiped_to_empty).unwrap();
        assert_ne!(third.root, second.root);
        assert_eq!(third.entries, 1);
        assert_eq!(qmdb.storage(hashed_address, slot_a).unwrap(), Some(U256::from(13)));
        assert_eq!(qmdb.storage(hashed_address, slot_b).unwrap(), None);
    }

    #[test]
    fn commits_cached_block_mutations_and_rewinds_on_root_mismatch() {
        let tempdir = tempfile::tempdir().unwrap();
        let qmdb = QmdbState::open(QmdbConfig::new(tempdir.path())).unwrap();
        let genesis = qmdb
            .commit_block(
                QmdbBlock { number: 0, hash: B256::with_last_byte(1), parent_hash: B256::ZERO },
                HashedPostState::default(),
            )
            .unwrap();

        let hashed_address = B256::repeat_byte(0x11);
        let hashed_slot = B256::repeat_byte(0x22);
        let account = Account { nonce: 1, balance: U256::from(100), bytecode_hash: None };
        let mutations = vec![
            (account_key(&hashed_address), Some(encode_account(&account))),
            (storage_key(&hashed_address, &hashed_slot), Some(encode_storage(U256::from(200)))),
        ];
        let overlay = qmdb.overlay_mutations(mutations.clone()).unwrap();
        let block =
            QmdbBlock { number: 1, hash: B256::with_last_byte(2), parent_hash: genesis.hash };
        let head = qmdb.commit_block_mutations(block, overlay.root, mutations).unwrap();

        assert_eq!(head.root, overlay.root);
        assert_eq!(qmdb.head().unwrap(), Some(head));
        assert_eq!(qmdb.account(hashed_address).unwrap(), Some(account));
        assert_eq!(qmdb.storage(hashed_address, hashed_slot).unwrap(), Some(U256::from(200)));

        let bad_mutations = vec![(
            storage_key(&hashed_address, &hashed_slot),
            Some(encode_storage(U256::from(201))),
        )];
        let bad_block =
            QmdbBlock { number: 2, hash: B256::with_last_byte(3), parent_hash: head.hash };
        let error = qmdb
            .commit_block_mutations(bad_block, B256::repeat_byte(0xff), bad_mutations)
            .unwrap_err();
        assert!(matches!(error, QmdbError::CanonicalRootMismatch { .. }));
        assert_eq!(qmdb.head().unwrap(), Some(head));
        assert_eq!(qmdb.root().unwrap(), head.root);
        assert_eq!(qmdb.storage(hashed_address, hashed_slot).unwrap(), Some(U256::from(200)));
    }

    #[test]
    fn qmdb_chain_commits_real_holesky_blocks() {
        let tempdir = tempfile::tempdir().unwrap();
        let config = QmdbConfig::new(tempdir.path()).with_partition_prefix("holesky");
        let qmdb = QmdbState::open(config.clone()).unwrap();
        let block1 =
            decode_holesky_block(include_str!("../../../engine/tree/test-data/holesky/1.rlp"));
        let block2 =
            decode_holesky_block(include_str!("../../../engine/tree/test-data/holesky/2.rlp"));

        assert_eq!(block2.parent_hash(), block1.hash());

        qmdb.commit_block(
            QmdbBlock { number: 0, hash: block1.parent_hash(), parent_hash: B256::ZERO },
            HashedPostState::default(),
        )
        .unwrap();
        let head1 = qmdb
            .commit_block(
                QmdbBlock {
                    number: block1.number(),
                    hash: block1.hash(),
                    parent_hash: block1.parent_hash(),
                },
                block_state(&block1),
            )
            .unwrap();
        let head2 = qmdb
            .commit_block(
                QmdbBlock {
                    number: block2.number(),
                    hash: block2.hash(),
                    parent_hash: block2.parent_hash(),
                },
                block_state(&block2),
            )
            .unwrap();

        assert_eq!(qmdb.head().unwrap(), Some(head2));
        assert_eq!(head2.number, block2.number());
        assert_ne!(head2.root, head1.root);

        drop(qmdb);
        let reopened = QmdbState::open(config).unwrap();
        assert_eq!(reopened.head().unwrap(), Some(head2));
        assert_eq!(reopened.root().unwrap(), head2.root);
    }

    #[test]
    fn commits_reopens_and_rewinds_transaction_shaped_blocks() {
        commit_reopen_and_rewind_blocks(4_096, 512, 2_048);
    }

    #[test]
    fn previews_pending_children_without_changing_the_durable_head() {
        let directory = tempfile::tempdir().unwrap();
        let state = QmdbState::open(QmdbConfig::new(directory.path())).unwrap();
        let mut parent_hash = B256::ZERO;
        state
            .commit_block(
                QmdbBlock { number: 0, hash: parent_hash, parent_hash },
                HashedPostState::default(),
            )
            .unwrap();
        let mut pending = Vec::new();
        for number in 1..=5 {
            let mutations = transaction_shaped_state(number);
            let preview = state.preview(parent_hash, mutations.clone()).unwrap();
            let block = QmdbBlock { number, hash: synthetic_block_hash(number), parent_hash };
            state.remember(block, preview.root).unwrap();
            pending.push((block, mutations, preview.root));
            parent_hash = block.hash;
        }
        assert_eq!(state.head().unwrap().unwrap().number, 0);
        for (block, mutations, expected_root) in pending {
            let committed = state.commit_block(block, mutations).unwrap();
            assert_eq!(committed.root, expected_root);
        }
    }

    #[test]
    #[ignore = "100k QMDb debug-mode commit/reopen/rewind harness; run explicitly for long-chain validation"]
    fn commits_reopens_and_rewinds_100k_transaction_shaped_blocks() {
        commit_reopen_and_rewind_blocks(100_000, 1_000, 50_000);
    }

    fn commit_reopen_and_rewind_blocks(blocks: u64, chunk: u64, rewind_to: u64) {
        let tempdir = tempfile::tempdir().unwrap();
        let config = QmdbConfig::new(tempdir.path()).with_partition_prefix("long-chain");
        let qmdb = QmdbState::open(config.clone()).unwrap();

        let mut parent_hash = B256::ZERO;
        qmdb.commit_block(
            QmdbBlock { number: 0, hash: parent_hash, parent_hash: B256::ZERO },
            HashedPostState::default(),
        )
        .unwrap();
        let mut head = None;
        for start in (1..=blocks).step_by(chunk as usize) {
            let end = (start + chunk - 1).min(blocks);
            let mut batch = Vec::with_capacity((end - start + 1) as usize);
            for number in start..=end {
                let hash = synthetic_block_hash(number);
                batch.push((
                    QmdbBlock { number, hash, parent_hash },
                    transaction_shaped_state(number),
                ));
                parent_hash = hash;
            }
            head = qmdb.commit_blocks(batch).unwrap();
        }

        let final_head = head.unwrap();
        assert_eq!(final_head.number, blocks);
        assert_eq!(qmdb.head().unwrap(), Some(final_head));

        drop(qmdb);
        let reopened = QmdbState::open(config.clone()).unwrap();
        assert_eq!(reopened.head().unwrap(), Some(final_head));
        assert_eq!(reopened.root().unwrap(), final_head.root);

        let rewound = reopened.rewind_to_block(rewind_to).unwrap().unwrap();
        assert_eq!(rewound.number, rewind_to);
        assert_eq!(reopened.head().unwrap(), Some(rewound));

        drop(reopened);
        let reopened = QmdbState::open(config).unwrap();
        assert_eq!(reopened.head().unwrap(), Some(rewound));
        assert_eq!(reopened.root().unwrap(), rewound.root);
    }

    fn decode_holesky_block(rlp: &str) -> reth_primitives_traits::SealedBlock<Block> {
        let data = Bytes::from_str(rlp).unwrap();
        Block::decode(&mut data.as_ref()).unwrap().seal_slow()
    }

    fn block_state(block: &reth_primitives_traits::SealedBlock<Block>) -> HashedPostState {
        let hashed_address = keccak256(block.beneficiary());
        let slot = keccak256(block.hash());
        let account = Account {
            nonce: block.number(),
            balance: U256::from(block.gas_limit()),
            bytecode_hash: None,
        };
        HashedPostState::default().with_accounts([(hashed_address, Some(account))]).with_storages([
            (hashed_address, HashedStorage::from_iter([(slot, U256::from(block.timestamp()))])),
        ])
    }

    fn synthetic_block_hash(number: u64) -> B256 {
        keccak256(number.to_be_bytes())
    }

    fn transaction_shaped_state(number: u64) -> HashedPostState {
        let sender = keccak256([number.to_be_bytes(), 0u64.to_be_bytes()].concat());
        let receiver = keccak256([number.to_be_bytes(), 1u64.to_be_bytes()].concat());
        let slot = keccak256([number.to_be_bytes(), 2u64.to_be_bytes()].concat());
        let sender_account =
            Account { nonce: number, balance: U256::from(number * 3), bytecode_hash: None };
        let receiver_account =
            Account { nonce: 0, balance: U256::from(number * 7), bytecode_hash: None };

        HashedPostState::default()
            .with_accounts([(sender, Some(sender_account)), (receiver, Some(receiver_account))])
            .with_storages([(
                receiver,
                HashedStorage::from_iter([(slot, U256::from(number * 11))]),
            )])
    }
}
