//! Incremental, content-addressed checkpoints. Block references are published only after
//! the corresponding trie nodes and root record have been durably synced.

use crate::{
    migration::{ArtifactError, Artifacts, DecodeLimits},
    Blake3Hasher, Proof, StateTree, TreeHasher,
};
use alloy_primitives::B256;
use pbt::persistence::{NodeDatabase, NodeError, STORE_VERSION};
use reth_lru::LruMap;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

const HASH_BYTES: usize = 32;
const CHECKPOINT_LENGTH_BYTES: usize = 8;
const LEGACY_CHECKPOINT_VERSION: u32 = 2;
const PREIMAGE_ACCOUNT_BYTES: usize = 24;
const SNAPSHOT_ACCOUNT_MAX_BYTES: usize = 130;
const SNAPSHOT_LEAF_MAX_BYTES: usize = 68;
const SNAPSHOT_TRAILER_BYTES: usize = 1 + HASH_BYTES;
// Cold reconstructions retain independent full trees; keep only recent parents and payloads.
const STATE_CACHE_CAPACITY: u32 = 8;
const STATE_CACHE_BYTES: usize = 1024 * 1024 * 1024;
const BLOCK_NUMBER_VERSION: u32 = 1;
const BLOCK_NUMBER_BYTES: usize = 4 + 8 + HASH_BYTES;

/// A durable, incremental trie store for migration replay and fork checkpoints.
///
/// Unchanged subtrees are shared on disk. Checkpoints remain until the caller explicitly
/// prunes blocks proven obsolete by finality. Cold loads reconstruct an in-memory state;
/// this does not replace Reth's historical-state providers.
#[derive(Debug)]
pub struct SnapshotStore<H = Blake3Hasher> {
    path: PathBuf,
    database: Arc<NodeDatabase>,
    cache: Mutex<LruMap<B256, StateTree<H>>>,
    cache_generation: AtomicU64,
    cache_bytes: usize,
    limits: DecodeLimits,
    native_limits: DecodeLimits,
}

impl<H: TreeHasher> SnapshotStore<H> {
    /// Opens a checkpoint directory. Handles in this process share the native database lock.
    pub fn open(path: impl Into<PathBuf>, limits: DecodeLimits) -> Result<Self, StoreError> {
        Self::open_with_limits(path, limits, limits)
    }

    /// Opens a store with separate untrusted-artifact and live native-state budgets.
    /// Use unrestricted native limits only for locally executed or authenticated state.
    pub fn open_with_limits(
        path: impl Into<PathBuf>,
        limits: DecodeLimits,
        native_limits: DecodeLimits,
    ) -> Result<Self, StoreError> {
        let path = path.into();
        reth_fs_util::create_dir_all(path.join("states"))?;
        reth_fs_util::create_dir_all(path.join("blocks"))?;
        reth_fs_util::create_dir_all(path.join("numbers"))?;
        let database = NodeDatabase::open(&path)?;
        Ok(Self {
            path,
            database,
            cache: Mutex::new(LruMap::new(STATE_CACHE_CAPACITY)),
            cache_generation: AtomicU64::new(0),
            cache_bytes: STATE_CACHE_BYTES,
            limits,
            native_limits,
        })
    }

    /// Returns the checkpoint directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads and verifies state using its trusted PBT root.
    pub fn load(&self, root: B256) -> Result<StateTree<H>, StoreError> {
        if self.database.mutation.is_poisoned() {
            return Err(StoreError::Poisoned);
        }
        if let Some(state) = self.cache()?.get(&root) {
            return Ok(state.clone());
        }
        let writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        // A concurrent writer or cold reader may have populated the cache while we waited.
        if let Some(state) = self.cache()?.get(&root) {
            return Ok(state.clone());
        }
        let Some(count) = self.native_count(root)? else {
            // Legacy upgrades and interrupted marker publication require mutation.
            let state = self.load_state(root)?;
            self.cache_candidate(&state)?;
            return Ok(state)
        };
        let _lease = self.database.pin_roots(&[root])?;
        drop(writer);
        let state = self.database.load::<H>(root, count, self.native_limits)?;
        self.cache_candidate(&state)?;
        Ok(state)
    }

    /// Persists a state before returning its root. No block reference is implied.
    pub fn save(&self, state: &StateTree<H>) -> Result<B256, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        self.save_state(state)
    }

    /// Publishes a block-hash-to-PBT-root reference after durably saving its state.
    ///
    /// This also supports pre-activation blocks whose header commits to the MPT instead.
    pub fn save_block(&self, block: B256, state: &StateTree<H>) -> Result<B256, StoreError> {
        self.save_block_inner(block, None, state)
    }

    /// Saves a checkpoint with its validated block height for finality retention.
    /// The caller must obtain both `block` and `number` from the same accepted header.
    /// Height metadata is checksummed against the block and state root and synced
    /// before the block reference becomes visible. Existing reference files stay compatible.
    pub fn save_block_with_number(
        &self,
        block: B256,
        number: u64,
        state: &StateTree<H>,
    ) -> Result<B256, StoreError> {
        self.save_block_inner(block, Some(number), state)
    }

    fn save_block_inner(
        &self,
        block: B256,
        number: Option<u64>,
        state: &StateTree<H>,
    ) -> Result<B256, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let reference = self.path.join("blocks").join(block.to_string());
        if reference
            .try_exists()
            .map_err(|error| reth_fs_util::FsPathError::metadata(error, &reference))?
        {
            let previous = self.block_root(block)?;
            if previous != state.root() && self.database.is_pinned(previous)? {
                return Err(StoreError::PinnedCheckpoint(previous));
            }
        }
        let root = self.save_state(state)?;
        let number_path = self.path.join("numbers").join(block.to_string());
        if let Some(number) = number {
            let encoded = number.to_be_bytes();
            let checksum =
                H::hash(&[b"pbt-block-number-v1", block.as_slice(), root.as_slice(), &encoded]);
            reth_fs_util::atomic_write_file(&number_path, |file| {
                file.write_all(&BLOCK_NUMBER_VERSION.to_be_bytes())?;
                file.write_all(&encoded)?;
                file.write_all(checksum.as_slice())
            })?;
        } else if number_path
            .try_exists()
            .map_err(|error| reth_fs_util::FsPathError::metadata(error, &number_path))?
        {
            reth_fs_util::remove_file(&number_path)?;
        }
        reth_fs_util::atomic_write_file(
            &self.path.join("blocks").join(block.to_string()),
            |file| file.write_all(root.as_slice()),
        )?;
        Ok(root)
    }

    /// Returns the locally recorded height, or `None` for a legacy reference.
    /// Malformed, corrupt or incorrectly bound metadata is an error.
    pub fn block_number(&self, block: B256) -> Result<Option<u64>, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        self.block_number_inner(block)
    }

    fn block_number_inner(&self, block: B256) -> Result<Option<u64>, StoreError> {
        let path = self.path.join("numbers").join(block.to_string());
        if !path.try_exists().map_err(|error| reth_fs_util::FsPathError::metadata(error, &path))? {
            return Ok(None);
        }
        let bytes = read_bounded(&path, BLOCK_NUMBER_BYTES)?;
        if bytes.len() != BLOCK_NUMBER_BYTES || bytes[..4] != BLOCK_NUMBER_VERSION.to_be_bytes() {
            return Err(ArtifactError::Invalid("invalid PBT block number metadata").into());
        }
        let mut encoded = [0; 8];
        encoded.copy_from_slice(&bytes[4..12]);
        let root = self.block_root(block)?;
        let checksum =
            H::hash(&[b"pbt-block-number-v1", block.as_slice(), root.as_slice(), &encoded]);
        if bytes[12..] != checksum.as_slice()[..] {
            return Err(ArtifactError::Invalid("invalid PBT block number checksum").into());
        }
        Ok(Some(u64::from_be_bytes(encoded)))
    }

    /// Resolves an accepted block's shadow or canonical PBT state.
    pub fn load_block(&self, block: B256) -> Result<StateTree<H>, StoreError> {
        self.load(self.block_root(block)?)
    }

    /// Re-verifies an accepted block's durable checkpoint without using the state cache.
    ///
    /// Call before deleting a fallback commitment store. Missing or corrupt block references
    /// or native state are errors. Recoverable checkpoint markers are repaired only after
    /// verifying native state. Returns the authenticated PBT root.
    pub fn verify_block(&self, block: B256) -> Result<B256, StoreError> {
        self.with_verified_blocks(&[block], |roots| Ok(roots[0]))
    }

    /// Re-authenticates durable checkpoints, then pins their roots throughout `action`.
    /// Authentication finishes before the callback acquires external database writers.
    /// No native mutation lock is held in the callback, avoiding lock inversion with
    /// staged database writers that publish PBT checkpoints. Pruning respects the pins.
    pub fn with_verified_blocks<T, E: From<StoreError>>(
        &self,
        blocks: &[B256],
        action: impl FnOnce(&[B256]) -> Result<T, E>,
    ) -> Result<T, E> {
        let (roots, counts, _lease) = {
            let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
            let roots = blocks
                .iter()
                .map(|block| self.block_root(*block))
                .collect::<Result<Vec<_>, _>>()?;
            let counts = roots
                .iter()
                .map(|root| self.checkpoint_count(*root))
                .collect::<Result<Vec<_>, _>>()?;
            let lease = self.database.pin_roots(&roots).map_err(StoreError::from)?;
            (roots, counts, lease)
        };
        for (root, count) in roots.iter().zip(counts) {
            self.database.load::<H>(*root, count, self.native_limits).map_err(StoreError::from)?;
        }
        action(&roots)
    }

    /// Imports bounded artifacts against a trusted finalized MPT header and checkpoints them.
    ///
    /// The caller must obtain `block` and `anchor_mpt_root` from its validated, finalized header
    /// chain. Both artifacts are authenticated before any checkpoint or block reference is saved.
    pub fn import_anchor(
        &self,
        block: B256,
        anchor_mpt_root: B256,
        snapshot_path: &Path,
        preimages_path: &Path,
    ) -> Result<B256, StoreError> {
        let artifacts = Artifacts {
            snapshot: read_bounded(snapshot_path, snapshot_byte_limit(self.limits)?)?,
            preimages: read_bounded(preimages_path, preimage_byte_limit(self.limits)?)?,
        };
        let state = artifacts.verify::<H>(anchor_mpt_root, self.limits)?;
        self.save_block(block, &state)
    }

    /// Converts retained MPT state and publishes it only after authenticating the anchor root.
    ///
    /// `source` must be pinned to the caller's finalized anchor. The preimage file is bounded
    /// before reading, then matched exactly to source leaves. Cancellation never publishes a
    /// checkpoint. Retained code-store entries not referenced by the source are ignored.
    #[cfg(feature = "conversion")]
    pub fn convert_anchor(
        &self,
        block: B256,
        anchor_mpt_root: B256,
        source: &impl crate::conversion::HashedCursorFactory,
        code_store: &impl crate::conversion::BytecodeReader,
        preimages_path: &Path,
        should_stop: impl FnMut() -> bool,
    ) -> Result<B256, StoreError> {
        let preimages = read_bounded(preimages_path, preimage_byte_limit(self.limits)?)?;
        let state = crate::conversion::convert_mpt::<H>(
            source,
            code_store,
            &preimages,
            anchor_mpt_root,
            self.limits,
            should_stop,
        )?;
        self.save_block(block, &state)
    }

    /// Deletes explicitly obsolete block references and their otherwise unreferenced states.
    ///
    /// The caller must authenticate the finalized checkpoint against its validated header and prove
    /// that each `obsolete_blocks` entry is obsolete under it; this store has no header ancestry
    /// information. Every other block reference
    /// and its state are retained, including competing branches and states shared with an obsolete
    /// block. Root-only checkpoints outside the obsolete candidates are also retained.
    ///
    /// All references, candidate states, and the durable finalized state are checked before the
    /// first deletion. A failure during deletion may leave obsolete files behind, but never removes
    /// a retained state's checkpoint. The finalized block cannot itself be pruned.
    pub fn prune_finalized(
        &self,
        finalized: B256,
        obsolete_blocks: &[B256],
    ) -> Result<PruneOutcome, StoreError> {
        self.prune(finalized, obsolete_blocks, false)
    }

    /// Prunes proven obsolete blocks and every checkpoint with no remaining block reference.
    /// Unlike `prune_finalized`, this also reclaims legacy discarded payload roots.
    /// Call only once the finalized reference is authenticated and all retained forks
    /// have durable block references. Active leases are retained; in-memory snapshots
    /// remain valid after pruning. Call `collect` afterwards to reclaim native nodes.
    pub fn prune_finalized_with_orphans(
        &self,
        finalized: B256,
        obsolete_blocks: &[B256],
    ) -> Result<PruneOutcome, StoreError> {
        self.prune(finalized, obsolete_blocks, true)
    }

    fn prune(
        &self,
        finalized: B256,
        obsolete_blocks: &[B256],
        orphans: bool,
    ) -> Result<PruneOutcome, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let mut obsolete: BTreeSet<_> = obsolete_blocks.iter().copied().collect();
        if obsolete.contains(&finalized) {
            return Err(StoreError::PruneFinalized);
        }
        let blocks_path = self.path.join("blocks");
        let mut references = BTreeMap::new();
        for entry in reth_fs_util::read_dir(&blocks_path)? {
            let entry =
                entry.map_err(|error| reth_fs_util::FsPathError::read_dir(error, &blocks_path))?;
            if entry.path().extension().is_some_and(|extension| extension == "tmp") {
                continue;
            }
            let name = entry.file_name();
            let name =
                name.to_str().ok_or(ArtifactError::Invalid("invalid block checkpoint filename"))?;
            let block = name
                .parse::<B256>()
                .map_err(|_| ArtifactError::Invalid("invalid block checkpoint filename"))?;
            if name != block.to_string() {
                return Err(ArtifactError::Invalid("invalid block checkpoint filename").into());
            }
            let root = self.block_root(block)?;
            references.insert(block, root);
        }
        for block in obsolete.clone() {
            let root = *references.get(&block).ok_or(StoreError::MissingBlockReference(block))?;
            if self.database.is_pinned(root)? {
                if !orphans {
                    return Err(StoreError::PinnedCheckpoint(root));
                }
                obsolete.remove(&block);
            }
        }
        let finalized_root =
            *references.get(&finalized).ok_or(StoreError::MissingBlockReference(finalized))?;
        // Bypass the cache so the one checkpoint pruning must preserve is verified on disk.
        if orphans {
            self.checkpoint_count(finalized_root)?;
        } else {
            self.load_state(finalized_root)?;
        }
        let mut candidate_roots = BTreeSet::new();
        for block in &obsolete {
            let root = *references.get(block).ok_or(StoreError::MissingBlockReference(*block))?;
            if candidate_roots.insert(root) && !orphans {
                self.load_state(root)?;
            }
        }
        let retained_roots: BTreeSet<_> = references
            .iter()
            .filter(|(block, _)| !obsolete.contains(*block))
            .map(|(_, root)| *root)
            .collect();
        if orphans {
            candidate_roots.extend(self.database.roots().collect::<Result<Vec<_>, _>>()?);
            for entry in reth_fs_util::read_dir(self.path.join("states"))? {
                let entry = entry.map_err(|error| {
                    reth_fs_util::FsPathError::read_dir(error, self.path.join("states"))
                })?;
                if entry.path().extension().is_some_and(|extension| extension == "tmp") {
                    continue;
                }
                let name = entry.file_name();
                let name = name
                    .to_str()
                    .ok_or(ArtifactError::Invalid("invalid state checkpoint filename"))?;
                let root = name
                    .parse::<B256>()
                    .map_err(|_| ArtifactError::Invalid("invalid state checkpoint filename"))?;
                if name != root.to_string() {
                    return Err(ArtifactError::Invalid("invalid state checkpoint filename").into());
                }
                candidate_roots.insert(root);
            }
        }
        for root in &retained_roots {
            let path = self.state_path(*root);
            if !reth_fs_util::metadata(&path)?.is_file() {
                return Err(ArtifactError::Invalid("invalid retained state checkpoint").into());
            }
        }
        let mut outcome = PruneOutcome::default();
        // Remove references first. Any interrupted prune leaves every surviving reference usable.
        for block in &obsolete {
            reth_fs_util::remove_file(blocks_path.join(block.to_string()))?;
            outcome.block_references += 1;
        }
        // Persist reference removal before any state file can disappear after a crash.
        reth_fs_util::open(&blocks_path)?
            .sync_all()
            .map_err(|error| reth_fs_util::FsPathError::fsync(error, &blocks_path))?;
        let numbers_path = self.path.join("numbers");
        for block in &obsolete {
            let path = numbers_path.join(block.to_string());
            if path
                .try_exists()
                .map_err(|error| reth_fs_util::FsPathError::metadata(error, &path))?
            {
                reth_fs_util::remove_file(path)?;
            }
        }
        if orphans {
            for entry in reth_fs_util::read_dir(&numbers_path)? {
                let entry = entry
                    .map_err(|error| reth_fs_util::FsPathError::read_dir(error, &numbers_path))?;
                if entry.path().extension().is_some_and(|extension| extension == "tmp") {
                    continue;
                }
                let name = entry.file_name();
                let name =
                    name.to_str().ok_or(ArtifactError::Invalid("invalid block number filename"))?;
                let block = name
                    .parse::<B256>()
                    .map_err(|_| ArtifactError::Invalid("invalid block number filename"))?;
                if name != block.to_string() {
                    return Err(ArtifactError::Invalid("invalid block number filename").into());
                }
                if !references.contains_key(&block) {
                    reth_fs_util::remove_file(entry.path())?;
                }
            }
        }
        reth_fs_util::open(&numbers_path)?
            .sync_all()
            .map_err(|error| reth_fs_util::FsPathError::fsync(error, &numbers_path))?;
        for root in candidate_roots.difference(&retained_roots) {
            if self.database.is_pinned(*root)? {
                continue;
            }
            let path = self.state_path(*root);
            if path
                .try_exists()
                .map_err(|error| reth_fs_util::FsPathError::metadata(error, &path))?
            {
                reth_fs_util::remove_file(path)?;
            }
            self.database.remove_root(*root)?;
            outcome.state_files += 1;
        }
        let states_path = self.path.join("states");
        reth_fs_util::open(&states_path)?
            .sync_all()
            .map_err(|error| reth_fs_util::FsPathError::fsync(error, &states_path))?;
        if !orphans {
            self.database.collect::<H>()?;
        }
        Ok(outcome)
    }

    /// Returns whether the store has no registered roots, legacy checkpoints, or block references.
    ///
    /// Includes durable native roots whose file publication was interrupted. Filesystem or database
    /// errors are returned rather than treating an unreadable directory as empty.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        if self.database.has_roots()? {
            return Ok(false);
        }
        for name in ["states", "blocks", "numbers"] {
            let path = self.path.join(name);
            if reth_fs_util::read_dir(&path)?
                .next()
                .transpose()
                .map_err(|error| reth_fs_util::FsPathError::read_dir(error, &path))?
                .is_some()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Changes the estimated byte budget for cached full-state snapshots.
    /// Zero disables caching. Existing entries are dropped before returning.
    pub fn set_cache_budget(&mut self, bytes: usize) -> Result<(), StoreError> {
        self.cache.get_mut().map_err(|_| StoreError::Poisoned)?.clear();
        self.cache_bytes = bytes;
        Ok(())
    }

    /// Caches a payload candidate when it fits the byte budget, without publishing
    /// a durable root or block reference. Disabled caching or eviction never changes
    /// durable state; callers must validate accepted blocks before building on them.
    pub fn cache_candidate(&self, state: &StateTree<H>) -> Result<(), StoreError> {
        let mut cache = self.cache()?;
        cache_state(&mut cache, state.root(), state.clone(), self.cache_bytes);
        Ok(())
    }

    /// Lists checkpoint block hashes without loading their states. A malformed
    /// filename fails the operation; callers authenticate header ancestry separately.
    pub fn block_hashes(&self) -> Result<Vec<B256>, StoreError> {
        let path = self.path.join("blocks");
        let mut blocks = Vec::new();
        for entry in reth_fs_util::read_dir(&path)? {
            let entry = entry.map_err(|error| reth_fs_util::FsPathError::read_dir(error, &path))?;
            if entry.path().extension().is_some_and(|extension| extension == "tmp") {
                continue;
            }
            let name = entry.file_name();
            let name =
                name.to_str().ok_or(ArtifactError::Invalid("invalid block checkpoint filename"))?;
            let block = name
                .parse::<B256>()
                .map_err(|_| ArtifactError::Invalid("invalid block checkpoint filename"))?;
            if name != block.to_string() {
                return Err(ArtifactError::Invalid("invalid block checkpoint filename").into());
            }
            blocks.push(block);
        }
        Ok(blocks)
    }

    /// Authenticates raw-key proofs directly from native paths, without restoring
    /// complete account/storage/code metadata. Legacy checkpoints are upgraded first.
    /// Holds the mutation lock until every proof is built, so pruning cannot race reads.
    pub fn proof_block(
        &self,
        block: B256,
        keys: &[impl AsRef<[u8]>],
    ) -> Result<BlockProofs, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let root = self.block_root(block)?;
        self.checkpoint_count(root)?;
        Ok(BlockProofs {
            root,
            proofs: keys
                .iter()
                .map(|key| self.database.proof::<H>(root, key.as_ref()).map_err(StoreError::from))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Returns a durable checkpoint root after checking its marker and native root
    /// record. Locally recorded heights also authenticate the reference checksum.
    /// Legacy references receive full authentication before returning.
    pub fn checkpoint_root(&self, block: B256) -> Result<B256, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let root = self.block_root(block)?;
        self.checkpoint_count(root)?;
        if self.block_number_inner(block)?.is_none() {
            self.load_state(root)?;
        }
        Ok(root)
    }

    /// Runs a short publication check and callback while a modern native reference
    /// cannot be replaced or pruned. Returns `None` if a legacy upgrade or repair is
    /// required; do that outside external write transactions. This checks durable
    /// publication, not complete state reconstruction. The callback must not re-enter
    /// mutation methods on this store.
    pub fn with_published_block<T, E: From<StoreError>>(
        &self,
        block: B256,
        action: impl FnOnce(B256) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let root = self.block_root(block)?;
        if self.native_count(root)?.is_none() || self.block_number_inner(block)?.is_none() {
            return Ok(None);
        }
        action(root).map(Some)
    }

    /// Reclaims nodes unregistered by orphan pruning, yielding between sweep batches.
    /// Does not hold the mutation lock across marking or the complete sweep. Returns
    /// false when another collector is active. Reference pruning must run separately.
    pub fn collect(&self) -> Result<bool, StoreError> {
        Ok(self.database.collect_cooperatively::<H>()?)
    }

    fn native_count(&self, root: B256) -> Result<Option<usize>, StoreError> {
        let Some(native) = self.database.leaf_count(root)? else { return Ok(None) };
        let path = self.state_path(root);
        if !path.try_exists().map_err(|error| reth_fs_util::FsPathError::metadata(error, &path))? ||
            reth_fs_util::metadata(&path)?.len() != (CHECKPOINT_LENGTH_BYTES + 4) as u64
        {
            return Ok(None)
        }
        let bytes = read_bounded(&path, CHECKPOINT_LENGTH_BYTES + 4)?;
        if bytes.len() != CHECKPOINT_LENGTH_BYTES + 4 {
            return Err(ArtifactError::Truncated.into());
        }
        let mut count = [0; CHECKPOINT_LENGTH_BYTES];
        count.copy_from_slice(&bytes[..CHECKPOINT_LENGTH_BYTES]);
        let count =
            usize::try_from(u64::from_be_bytes(count)).map_err(|_| ArtifactError::ResourceLimit)?;
        if count > self.native_limits.max_leaves {
            return Err(ArtifactError::ResourceLimit.into());
        }
        if count != native {
            return Err(ArtifactError::Invalid("invalid PBT checkpoint marker").into());
        }
        if bytes[CHECKPOINT_LENGTH_BYTES..] == LEGACY_CHECKPOINT_VERSION.to_be_bytes() {
            return Ok(None)
        }
        if bytes[CHECKPOINT_LENGTH_BYTES..] != STORE_VERSION.to_be_bytes() {
            return Err(ArtifactError::Invalid("invalid PBT checkpoint marker").into());
        }
        Ok(Some(count))
    }

    fn checkpoint_count(&self, root: B256) -> Result<usize, StoreError> {
        match self.native_count(root)? {
            Some(count) => Ok(count),
            None => Ok(self.load_state(root)?.tree().len()),
        }
    }

    fn cache(&self) -> Result<MutexGuard<'_, LruMap<B256, StateTree<H>>>, StoreError> {
        if self.database.mutation.is_poisoned() {
            return Err(StoreError::Poisoned);
        }
        let mut cache = self.cache.lock().map_err(|_| StoreError::Poisoned)?;
        let generation = self.database.generation();
        if generation != self.cache_generation.load(Ordering::Relaxed) {
            let roots = cache.iter().map(|(root, _)| *root).collect::<Vec<_>>();
            for root in roots {
                if self.database.leaf_count(root)?.is_none() {
                    cache.remove(&root);
                }
            }
            self.cache_generation.store(generation, Ordering::Relaxed);
        }
        Ok(cache)
    }

    /// Reports whether a block reference exists, without authenticating its state.
    ///
    /// Call [`Self::load_block`] to verify it. A corrupt reference is not treated as missing.
    pub fn contains_block(&self, block: B256) -> Result<bool, StoreError> {
        let path = self.path.join("blocks").join(block.to_string());
        path.try_exists().map_err(|error| reth_fs_util::FsPathError::metadata(error, path).into())
    }

    fn state_path(&self, root: B256) -> PathBuf {
        self.path.join("states").join(root.to_string())
    }

    fn save_state(&self, state: &StateTree<H>) -> Result<B256, StoreError> {
        self.database.save(state, self.native_limits)?;
        #[cfg(test)]
        tests::pause_write();
        let root = state.root();
        self.write_marker(root, state.tree().len())?;
        self.cache_candidate(state)?;
        Ok(root)
    }

    fn block_root(&self, block: B256) -> Result<B256, StoreError> {
        let bytes = read_bounded(&self.path.join("blocks").join(block.to_string()), HASH_BYTES)?;
        if bytes.len() != HASH_BYTES {
            return Err(ArtifactError::Invalid("invalid block checkpoint reference").into());
        }
        Ok(B256::from_slice(&bytes))
    }

    fn write_marker(&self, root: B256, count: usize) -> Result<(), StoreError> {
        let count = u64::try_from(count).map_err(|_| ArtifactError::ResourceLimit)?;
        reth_fs_util::atomic_write_file(&self.state_path(root), |file| -> std::io::Result<()> {
            file.write_all(&count.to_be_bytes())?;
            file.write_all(&STORE_VERSION.to_be_bytes())
        })?;
        Ok(())
    }

    fn load_state(&self, root: B256) -> Result<StateTree<H>, StoreError> {
        let Some(count) = self.database.leaf_count(root)? else {
            // Legacy full-artifact checkpoints are verified before upgrading them in place.
            let state = self.load_legacy_state(root)?;
            self.database.save(&state, self.native_limits)?;
            self.write_marker(root, state.tree().len())?;
            return Ok(state);
        };
        let path = self.state_path(root);
        if path.try_exists().map_err(|error| reth_fs_util::FsPathError::metadata(error, &path))? {
            if reth_fs_util::metadata(&path)?.len() > (CHECKPOINT_LENGTH_BYTES + 4) as u64 {
                // Resume an upgrade interrupted after syncing nodes but before replacing the file.
                let state = self.load_legacy_state(root)?;
                self.write_marker(root, state.tree().len())?;
                return Ok(state);
            }
            let (mut file, _) = open_bounded(&path, CHECKPOINT_LENGTH_BYTES + 4)?;
            let mut bytes = [0; CHECKPOINT_LENGTH_BYTES];
            read_exact(&mut file, &mut bytes, &path)?;
            let stored = usize::try_from(u64::from_be_bytes(bytes))
                .map_err(|_| ArtifactError::ResourceLimit)?;
            if stored > self.native_limits.max_leaves {
                return Err(ArtifactError::ResourceLimit.into());
            }
            let mut version = [0; 4];
            read_exact(&mut file, &mut version, &path)?;
            check_eof(&mut file, &path)?;
            if stored != count {
                return Err(ArtifactError::Invalid("invalid PBT checkpoint marker").into());
            }
            if version == LEGACY_CHECKPOINT_VERSION.to_be_bytes() &&
                version != STORE_VERSION.to_be_bytes()
            {
                // Authenticate native nodes before upgrading the separately published marker.
                let state = self.database.load::<H>(root, count, self.native_limits)?;
                self.write_marker(root, count)?;
                return Ok(state)
            }
            if version != STORE_VERSION.to_be_bytes() {
                return Err(ArtifactError::Invalid("invalid PBT checkpoint marker").into());
            }
        } else {
            // A crash may sync the root record before publishing its small checkpoint marker.
            let state = self.database.load::<H>(root, count, self.native_limits)?;
            self.write_marker(root, count)?;
            return Ok(state);
        }
        Ok(self.database.load::<H>(root, count, self.native_limits)?)
    }

    fn load_legacy_state(&self, root: B256) -> Result<StateTree<H>, StoreError> {
        let path = self.state_path(root);
        let preimage_limit = preimage_byte_limit(self.limits)?;
        let snapshot_limit = snapshot_byte_limit(self.limits)?;
        let limit = CHECKPOINT_LENGTH_BYTES
            .checked_add(preimage_limit)
            .and_then(|limit| limit.checked_add(snapshot_limit))
            .ok_or(ArtifactError::ResourceLimit)?;
        let (mut file, length) = open_bounded(&path, limit)?;
        let mut prefix = [0; CHECKPOINT_LENGTH_BYTES];
        read_exact(&mut file, &mut prefix, &path)?;
        let preimage_length = usize::try_from(u64::from_be_bytes(prefix))
            .map_err(|_| ArtifactError::ResourceLimit)?;
        if preimage_length > preimage_limit {
            return Err(ArtifactError::ResourceLimit.into());
        }
        let snapshot_length = length
            .checked_sub(CHECKPOINT_LENGTH_BYTES)
            .and_then(|length| length.checked_sub(preimage_length))
            .ok_or(ArtifactError::Truncated)?;
        if snapshot_length > snapshot_limit {
            return Err(ArtifactError::ResourceLimit.into());
        }
        let mut artifacts =
            Artifacts { preimages: vec![0; preimage_length], snapshot: vec![0; snapshot_length] };
        read_exact(&mut file, &mut artifacts.preimages, &path)?;
        read_exact(&mut file, &mut artifacts.snapshot, &path)?;
        check_eof(&mut file, &path)?;
        Ok(artifacts.restore::<H>(root, self.limits)?)
    }
}

/// Files removed by an explicit finalized-checkpoint prune.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    /// Number of obsolete block references deleted.
    pub block_references: usize,
    /// Number of state files no remaining block reference uses that were deleted.
    pub state_files: usize,
}

/// Authenticated native proofs for one durable block checkpoint.
#[derive(Debug)]
pub struct BlockProofs {
    /// PBT commitment resolved from the block reference.
    pub root: B256,
    /// Proofs in the same order as the requested keys.
    pub proofs: Vec<Proof>,
}

/// Checkpoint read/write or verification failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Native trie database operation failed.
    #[error(transparent)]
    Database(#[from] rocksdb::Error),
    /// Source MPT conversion or preimage matching failed.
    #[cfg(feature = "conversion")]
    #[error(transparent)]
    Conversion(#[from] crate::conversion::ConversionError),
    /// Filesystem operation failed, including its path.
    #[error(transparent)]
    Io(#[from] reth_fs_util::FsPathError),
    /// Checkpoint failed canonical artifact or root verification.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// A panicking store writer poisoned the serialization lock.
    #[error("PBT snapshot store lock poisoned")]
    Poisoned,
    /// A prune request attempted to remove the trusted finalized block itself.
    #[error("cannot prune the finalized PBT checkpoint")]
    PruneFinalized,
    /// A requested finalized or obsolete block has no checkpoint reference.
    #[error("PBT checkpoint reference is unavailable for block {0}")]
    MissingBlockReference(B256),
    /// A live root lease prevents checkpoint deletion or reference replacement.
    #[error("PBT checkpoint is pinned: {0}")]
    PinnedCheckpoint(B256),
}

impl From<NodeError> for StoreError {
    fn from(error: NodeError) -> Self {
        match error {
            NodeError::Database(error) => Self::Database(error),
            NodeError::Artifact(error) => Self::Artifact(error),
            NodeError::Io { path, source } => {
                Self::Io(reth_fs_util::FsPathError::metadata(source, path))
            }
            NodeError::Poisoned => Self::Poisoned,
        }
    }
}

fn cache_state<H: TreeHasher>(
    cache: &mut LruMap<B256, StateTree<H>>,
    root: B256,
    state: StateTree<H>,
    budget: usize,
) {
    if budget == 0 || state.estimated_memory_size() > budget {
        return;
    }
    // Cache eviction never removes the durable checkpoint needed by a fork.
    cache.insert(root, state);
    while cache
        .iter()
        .fold(0usize, |bytes, (_, state)| bytes.saturating_add(state.estimated_memory_size())) >
        budget
    {
        let Some((root, _)) = cache.peek_oldest() else { break };
        let root = *root;
        cache.remove(&root);
    }
}

fn preimage_byte_limit(limits: DecodeLimits) -> Result<usize, ArtifactError> {
    // Each account carries address[20] + slotCount[4], and each storage leaf at most one slot[32].
    limits
        .max_accounts
        .checked_mul(PREIMAGE_ACCOUNT_BYTES)
        .and_then(|accounts| limits.max_leaves.checked_mul(HASH_BYTES)?.checked_add(accounts))
        .ok_or(ArtifactError::ResourceLimit)
}

fn snapshot_byte_limit(limits: DecodeLimits) -> Result<usize, ArtifactError> {
    // A header uses at most 97 bytes excluding inline slot values; its storage marker uses 33.
    // Each leaf adds at most 34 bytes of group framing and 34 bytes for its index and value.
    limits
        .max_accounts
        .checked_mul(SNAPSHOT_ACCOUNT_MAX_BYTES)
        .and_then(|accounts| {
            limits.max_leaves.checked_mul(SNAPSHOT_LEAF_MAX_BYTES)?.checked_add(accounts)
        })
        .and_then(|records| records.checked_add(SNAPSHOT_TRAILER_BYTES))
        .ok_or(ArtifactError::ResourceLimit)
}

fn open_bounded(path: &Path, limit: usize) -> Result<(File, usize), StoreError> {
    let file = reth_fs_util::open(path)?;
    let metadata =
        file.metadata().map_err(|error| reth_fs_util::FsPathError::metadata(error, path))?;
    let length = usize::try_from(metadata.len()).map_err(|_| ArtifactError::ResourceLimit)?;
    if !metadata.is_file() || length > limit {
        return Err(ArtifactError::ResourceLimit.into());
    }
    Ok((file, length))
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, StoreError> {
    let (mut file, length) = open_bounded(path, limit)?;
    let mut bytes = vec![0; length];
    read_exact(&mut file, &mut bytes, path)?;
    check_eof(&mut file, path)?;
    Ok(bytes)
}

fn read_exact(file: &mut File, bytes: &mut [u8], path: &Path) -> Result<(), StoreError> {
    file.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ArtifactError::Truncated.into()
        } else {
            reth_fs_util::FsPathError::read(error, path).into()
        }
    })
}

fn check_eof(file: &mut File, path: &Path) -> Result<(), StoreError> {
    let mut trailing = [0];
    if file.read(&mut trailing).map_err(|error| reth_fs_util::FsPathError::read(error, path))? != 0
    {
        return Err(ArtifactError::Invalid("checkpoint file changed while reading").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AccountData;
    use alloy_primitives::{Address, U256};
    use std::{
        cell::RefCell,
        sync::{mpsc, TryLockError},
        thread,
        time::Duration,
    };

    const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

    thread_local! {
        static HASH_PAUSE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static WRITE_PAUSE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn pause_write() {
        if let Some(pause) = WRITE_PAUSE.with(|pause| pause.borrow_mut().take()) {
            pause();
        }
    }

    #[derive(Clone, Debug)]
    struct PausingHasher;

    impl TreeHasher for PausingHasher {
        fn hash(parts: &[&[u8]]) -> B256 {
            if let Some(pause) = HASH_PAUSE.with(|pause| pause.borrow_mut().take()) {
                pause();
            }
            Blake3Hasher::hash(parts)
        }
    }

    #[test]
    fn recently_loaded_parent_survives_checkpoint_eviction() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let states = sorted_states();
        let (newest, retained) = states.split_last().unwrap();
        for state in retained {
            store.save(state).unwrap();
        }
        let parent = retained[0].root();
        assert_eq!(store.load(parent).unwrap().root(), parent);
        store.save(newest).unwrap();
        assert_eq!(
            store.cache.lock().unwrap().len(),
            usize::try_from(STATE_CACHE_CAPACITY).unwrap()
        );
        // A cache miss must read this marker; a recently used parent remains authenticated in
        // memory.
        reth_fs_util::write(store.state_path(parent), [0; 7]).unwrap();
        assert_eq!(store.load(parent).unwrap().root(), parent);
        assert_eq!(store.load(retained[1].root()).unwrap().root(), retained[1].root());
        assert_eq!(
            store.cache.lock().unwrap().len(),
            usize::try_from(STATE_CACHE_CAPACITY).unwrap()
        );
    }

    #[test]
    fn refreshing_checkpoint_does_not_evict_another_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let states = sorted_states();
        let retained = &states[..usize::try_from(STATE_CACHE_CAPACITY).unwrap()];
        for state in retained {
            store.save(state).unwrap();
        }
        store.save(retained.last().unwrap()).unwrap();
        for state in retained {
            reth_fs_util::write(store.state_path(state.root()), [0; 7]).unwrap();
        }
        for state in retained {
            assert_eq!(store.load(state.root()).unwrap().root(), state.root());
        }
    }

    #[test]
    fn cached_parent_load_does_not_wait_for_database_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let state = account_state::<Blake3Hasher>(1);
        let root = store.save(&state).unwrap();
        thread::scope(|scope| {
            let writer = store.database.mutation.lock().unwrap();
            let (sender, receiver) = mpsc::channel();
            let store = &store;
            let reader =
                scope.spawn(move || sender.send(store.load(root).map(|state| state.root())));
            let result = receiver.recv_timeout(WAIT_TIMEOUT);
            drop(writer);
            reader.join().unwrap().unwrap();
            assert_eq!(result.unwrap().unwrap(), root);
        });
    }

    #[test]
    fn cached_parent_load_rejects_a_poisoned_database_mutation_lock() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let root = store.save(&account_state::<Blake3Hasher>(1)).unwrap();
        thread::scope(|scope| {
            let writer = scope.spawn(|| {
                let _writer = store.database.mutation.lock().unwrap();
                panic!("interrupted checkpoint mutation");
            });
            assert!(writer.join().is_err());
        });
        assert!(matches!(store.load(root), Err(StoreError::Poisoned)));
    }

    #[test]
    fn cached_parent_and_writer_do_not_wait_for_cold_reconstruction() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let cold = store.save(&account_state::<PausingHasher>(1)).unwrap();
        let hot = store.save(&account_state::<PausingHasher>(2)).unwrap();
        store.cache.lock().unwrap().remove(&cold);
        thread::scope(|scope| {
            let (started_sender, started_receiver) = mpsc::channel();
            let (resume_sender, resume_receiver) = mpsc::channel();
            let cold_store = &store;
            let cold_reader = scope.spawn(move || {
                HASH_PAUSE.with(|pause| {
                    *pause.borrow_mut() = Some(Box::new(move || {
                        started_sender.send(()).unwrap();
                        resume_receiver.recv_timeout(WAIT_TIMEOUT * 2).unwrap();
                    }));
                });
                cold_store.load(cold).unwrap().root()
            });
            started_receiver.recv_timeout(WAIT_TIMEOUT).unwrap();
            let (sender, receiver) = mpsc::channel();
            let hot_store = &store;
            let hot_reader =
                scope.spawn(move || sender.send(hot_store.load(hot).map(|state| state.root())));
            let result = receiver.recv_timeout(WAIT_TIMEOUT);
            let (writer_sender, writer_receiver) = mpsc::channel();
            let writer_store = &store;
            let writer =
                scope.spawn(move || writer_sender.send(writer_store.save(&account_state(3))));
            let written = writer_receiver.recv_timeout(WAIT_TIMEOUT);
            resume_sender.send(()).unwrap();
            assert_eq!(cold_reader.join().unwrap(), cold);
            hot_reader.join().unwrap().unwrap();
            writer.join().unwrap().unwrap();
            assert_eq!(result.unwrap().unwrap(), hot);
            assert_eq!(written.unwrap().unwrap(), account_state::<PausingHasher>(3).root());
        });
    }

    #[test]
    fn cached_parent_read_is_independent_of_checkpoint_io() {
        for publish_block in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
            let parent = account_state::<Blake3Hasher>(1);
            let root = store.save(&parent).unwrap();
            thread::scope(|scope| {
                let (started_sender, started_receiver) = mpsc::channel();
                let (resume_sender, resume_receiver) = mpsc::channel();
                let writer_store = &store;
                let writer = scope.spawn(move || {
                    WRITE_PAUSE.with(|pause| {
                        *pause.borrow_mut() = Some(Box::new(move || {
                            started_sender.send(()).unwrap();
                            resume_receiver.recv_timeout(WAIT_TIMEOUT * 2).unwrap();
                        }))
                    });
                    let state = account_state(2);
                    if publish_block {
                        writer_store
                            .save_block_with_number(B256::repeat_byte(2), 2, &state)
                            .unwrap();
                    } else {
                        writer_store.save(&state).unwrap();
                    }
                });
                started_receiver.recv_timeout(WAIT_TIMEOUT).unwrap();
                let (sender, receiver) = mpsc::channel();
                let reader_store = &store;
                let reader = scope
                    .spawn(move || sender.send(reader_store.load(root).map(|state| state.root())));
                let result = receiver.recv_timeout(WAIT_TIMEOUT);
                resume_sender.send(()).unwrap();
                writer.join().unwrap();
                reader.join().unwrap().unwrap();
                assert_eq!(result.unwrap().unwrap(), root);
            });
        }
    }

    #[test]
    fn candidate_roots_are_transient_and_cache_bytes_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let state = account_state::<Blake3Hasher>(1);
        store.set_cache_budget(state.estimated_memory_size()).unwrap();
        for value in 1..20 {
            let state = account_state(value);
            store.cache_candidate(&state).unwrap();
            assert_eq!(store.load(state.root()).unwrap().root(), state.root());
            assert_eq!(store.cache.lock().unwrap().len(), 1);
        }
        assert!(store.is_empty().unwrap());
        assert!(store.block_hashes().unwrap().is_empty());
        assert_eq!(reth_fs_util::read_dir(directory.path().join("states")).unwrap().count(), 0);
        store.set_cache_budget(0).unwrap();
        store.cache_candidate(&state).unwrap();
        assert!(store.cache.lock().unwrap().is_empty());
    }

    #[test]
    fn orphan_cleanup_invalidates_other_handles_and_keeps_retained_cache_entries() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let other =
            SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
        let old = account_state::<Blake3Hasher>(1);
        let finalized = account_state(2);
        let a = B256::repeat_byte(1);
        let b = B256::repeat_byte(2);
        store.save_block_with_number(a, 1, &old).unwrap();
        store.save_block_with_number(b, 2, &finalized).unwrap();
        for value in 3..23 {
            store.save(&account_state(value)).unwrap();
        }
        other.load(old.root()).unwrap();
        other.load(finalized.root()).unwrap();
        let interrupted =
            directory.path().join("numbers").join(B256::repeat_byte(0x80).to_string());
        reth_fs_util::write(&interrupted, [0; BLOCK_NUMBER_BYTES]).unwrap();
        let pruned = store.prune_finalized_with_orphans(b, &[a]).unwrap();
        assert!(store.collect().unwrap());
        assert_eq!(pruned, PruneOutcome { block_references: 1, state_files: 21 });
        assert_eq!(other.database.leaf_count(old.root()).unwrap(), None);
        assert!(matches!(other.load(old.root()), Err(StoreError::Io(_))));
        assert!(!other.cache.lock().unwrap().contains(&old.root()));
        assert!(other.cache.lock().unwrap().contains(&finalized.root()));
        reth_fs_util::write(store.state_path(finalized.root()), [0; 7]).unwrap();
        assert_eq!(other.load(finalized.root()).unwrap().root(), finalized.root());
        assert_eq!(store.block_hashes().unwrap(), vec![b]);
        assert_eq!(store.block_number(b).unwrap(), Some(2));
        assert_eq!(store.block_number(a).unwrap(), None);
        assert!(!interrupted.try_exists().unwrap());
    }

    #[test]
    fn import_limits_do_not_cap_native_chain_growth_or_native_proofs() {
        let directory = tempfile::tempdir().unwrap();
        let tight = DecodeLimits { max_accounts: 0, max_leaves: 1, ..Default::default() };
        let store =
            SnapshotStore::open_with_limits(directory.path(), tight, DecodeLimits::native())
                .unwrap();
        let state = account_state::<Blake3Hasher>(1);
        let block = B256::repeat_byte(1);
        store.save_block_with_number(block, 7, &state).unwrap();
        store.cache.lock().unwrap().clear();
        assert_eq!(store.load_block(block).unwrap().root(), state.root());
        let keys = state.tree().entries().map(|(key, _)| key.to_vec()).collect::<Vec<_>>();
        let proofs = store.proof_block(block, &keys).unwrap();
        assert_eq!(proofs.root, state.root());
        for (proof, key) in proofs.proofs.iter().zip(keys) {
            assert_eq!(proof, &state.tree().proof(&key));
        }
        assert!(matches!(
            Artifacts::from_state(&state).unwrap().restore::<Blake3Hasher>(state.root(), tight),
            Err(ArtifactError::ResourceLimit)
        ));
        store
            .with_verified_blocks::<_, StoreError>(&[block], |roots| {
                assert_eq!(roots, &[state.root()]);
                assert!(store.database.mutation.try_lock().is_ok());
                assert!(store.database.is_pinned(state.root())?);
                assert_eq!(
                    store.with_published_block::<_, StoreError>(block, |root| {
                        assert_eq!(root, state.root());
                        assert!(matches!(
                            store.database.mutation.try_lock(),
                            Err(TryLockError::WouldBlock)
                        ));
                        Ok(())
                    })?,
                    Some(())
                );
                assert!(matches!(store.save_block(block, &account_state(2)), Err(StoreError::PinnedCheckpoint(root)) if root == state.root()));
                Ok(())
            })
            .unwrap();
        let number_path = directory.path().join("numbers").join(block.to_string());
        let mut bytes = reth_fs_util::read(&number_path).unwrap();
        bytes[11] ^= 1;
        reth_fs_util::write(&number_path, bytes).unwrap();
        assert!(matches!(
            store.block_number(block),
            Err(StoreError::Artifact(ArtifactError::Invalid("invalid PBT block number checksum")))
        ));
    }

    #[test]
    fn verified_checkpoint_leases_prevent_pruning_and_release_on_error() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default()).unwrap();
        let old = account_state::<Blake3Hasher>(1);
        let tip = account_state(2);
        let a = B256::repeat_byte(1);
        let b = B256::repeat_byte(2);
        store.save_block_with_number(a, 1, &old).unwrap();
        store.save_block_with_number(b, 2, &tip).unwrap();
        let result = store.with_verified_blocks::<(), StoreError>(&[a], |_| {
            assert!(store.database.mutation.try_lock().is_ok());
            assert!(matches!(store.prune_finalized(b, &[a]), Err(StoreError::PinnedCheckpoint(root)) if root == old.root()));
            assert_eq!(store.prune_finalized_with_orphans(b, &[a])?, PruneOutcome::default());
            assert!(store.contains_block(a)?);
            Err(ArtifactError::Invalid("cancelled commit").into())
        });
        assert!(matches!(
            result,
            Err(StoreError::Artifact(ArtifactError::Invalid("cancelled commit")))
        ));
        assert!(!store.database.is_pinned(old.root()).unwrap());
        assert_eq!(
            store.prune_finalized(b, &[a]).unwrap(),
            PruneOutcome { block_references: 1, state_files: 1 }
        );
    }

    fn sorted_states() -> Vec<StateTree> {
        let mut states: Vec<_> = (0..=STATE_CACHE_CAPACITY).map(account_state).collect();
        states.sort_unstable_by_key(StateTree::root);
        states
    }

    fn account_state<H: TreeHasher>(balance: u32) -> StateTree<H> {
        let mut state = StateTree::new();
        state
            .set_account(
                Address::ZERO,
                AccountData { nonce: 1, balance: U256::from(balance), code: Default::default() },
            )
            .unwrap();
        state
    }
}
