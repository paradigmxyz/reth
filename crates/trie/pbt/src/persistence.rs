//! Incremental, content-addressed checkpoints. Block references are published only after
//! the corresponding trie nodes and root record have been durably synced.

use crate::{
    migration::{ArtifactError, Artifacts, DecodeLimits},
    Blake3Hasher, StateTree, TreeHasher,
};
use alloy_primitives::B256;
use pbt::persistence::{NodeDatabase, NodeError, STORE_VERSION};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const HASH_BYTES: usize = 32;
const CHECKPOINT_LENGTH_BYTES: usize = 8;
const PREIMAGE_ACCOUNT_BYTES: usize = 24;
const SNAPSHOT_ACCOUNT_MAX_BYTES: usize = 130;
const SNAPSHOT_LEAF_MAX_BYTES: usize = 68;
const SNAPSHOT_TRAILER_BYTES: usize = 1 + HASH_BYTES;

/// A durable, incremental trie store for migration replay and fork checkpoints.
///
/// Unchanged subtrees are shared on disk. Checkpoints remain until the caller explicitly
/// prunes blocks proven obsolete by finality. Cold loads reconstruct an in-memory state;
/// this does not replace Reth's historical-state providers.
#[derive(Debug)]
pub struct SnapshotStore<H = Blake3Hasher> {
    path: PathBuf,
    database: Arc<NodeDatabase>,
    cache: Mutex<BTreeMap<B256, StateTree<H>>>,
    limits: DecodeLimits,
}

impl<H: TreeHasher> SnapshotStore<H> {
    /// Opens a checkpoint directory. Handles in this process share the native database lock.
    pub fn open(path: impl Into<PathBuf>, limits: DecodeLimits) -> Result<Self, StoreError> {
        let path = path.into();
        reth_fs_util::create_dir_all(path.join("states"))?;
        reth_fs_util::create_dir_all(path.join("blocks"))?;
        let database = NodeDatabase::open(&path)?;
        Ok(Self { path, database, cache: Mutex::default(), limits })
    }

    /// Returns the checkpoint directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads and verifies state using its trusted PBT root.
    pub fn load(&self, root: B256) -> Result<StateTree<H>, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let mut cache = self.cache.lock().map_err(|_| StoreError::Poisoned)?;
        if let Some(state) = cache.get(&root) {
            return Ok(state.clone());
        }
        let state = self.load_state(root)?;
        cache_state(&mut cache, root, state.clone());
        Ok(state)
    }

    /// Persists a state before returning its root. No block reference is implied.
    pub fn save(&self, state: &StateTree<H>) -> Result<B256, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let mut cache = self.cache.lock().map_err(|_| StoreError::Poisoned)?;
        self.save_state(state, &mut cache)
    }

    /// Publishes a block-hash-to-PBT-root reference after durably saving its state.
    ///
    /// This also supports pre-activation blocks whose header commits to the MPT instead.
    pub fn save_block(&self, block: B256, state: &StateTree<H>) -> Result<B256, StoreError> {
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let mut cache = self.cache.lock().map_err(|_| StoreError::Poisoned)?;
        let root = self.save_state(state, &mut cache)?;
        reth_fs_util::atomic_write_file(
            &self.path.join("blocks").join(block.to_string()),
            |file| file.write_all(root.as_slice()),
        )?;
        Ok(root)
    }

    /// Resolves an accepted block's shadow or canonical PBT state.
    pub fn load_block(&self, block: B256) -> Result<StateTree<H>, StoreError> {
        self.load(self.block_root(block)?)
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
        let _writer = self.database.mutation.lock().map_err(|_| StoreError::Poisoned)?;
        let mut cache = self.cache.lock().map_err(|_| StoreError::Poisoned)?;
        let obsolete: BTreeSet<_> = obsolete_blocks.iter().copied().collect();
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
        let finalized_root =
            *references.get(&finalized).ok_or(StoreError::MissingBlockReference(finalized))?;
        // Bypass the cache so the one checkpoint pruning must preserve is verified on disk.
        self.load_state(finalized_root)?;
        let mut candidate_roots = BTreeSet::new();
        for block in &obsolete {
            let root = *references.get(block).ok_or(StoreError::MissingBlockReference(*block))?;
            if candidate_roots.insert(root) {
                self.load_state(root)?;
            }
        }
        let retained_roots: BTreeSet<_> = references
            .iter()
            .filter(|(block, _)| !obsolete.contains(*block))
            .map(|(_, root)| *root)
            .collect();
        for root in &retained_roots {
            let path = self.state_path(*root);
            if !reth_fs_util::metadata(&path)?.is_file() {
                return Err(ArtifactError::Invalid("invalid retained state checkpoint").into());
            }
        }
        let mut outcome = PruneOutcome::default();
        // Remove references first. Any interrupted prune leaves every surviving reference usable.
        for block in obsolete {
            reth_fs_util::remove_file(blocks_path.join(block.to_string()))?;
            outcome.block_references += 1;
        }
        // Persist reference removal before any state file can disappear after a crash.
        reth_fs_util::open(&blocks_path)?
            .sync_all()
            .map_err(|error| reth_fs_util::FsPathError::fsync(error, &blocks_path))?;
        for root in candidate_roots.difference(&retained_roots) {
            reth_fs_util::remove_file(self.state_path(*root))?;
            self.database.remove_root(*root)?;
            cache.remove(root);
            outcome.state_files += 1;
        }
        let states_path = self.path.join("states");
        reth_fs_util::open(&states_path)?
            .sync_all()
            .map_err(|error| reth_fs_util::FsPathError::fsync(error, &states_path))?;
        self.database.collect::<H>()?;
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
        for name in ["states", "blocks"] {
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

    fn save_state(
        &self,
        state: &StateTree<H>,
        cache: &mut BTreeMap<B256, StateTree<H>>,
    ) -> Result<B256, StoreError> {
        self.database.save(state, self.limits)?;
        let root = state.root();
        self.write_marker(root, state.tree().len())?;
        cache_state(cache, root, state.clone());
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
            self.database.save(&state, self.limits)?;
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
            if stored > self.limits.max_leaves {
                return Err(ArtifactError::ResourceLimit.into());
            }
            let mut version = [0; 4];
            read_exact(&mut file, &mut version, &path)?;
            check_eof(&mut file, &path)?;
            if stored != count || version != STORE_VERSION.to_be_bytes() {
                return Err(ArtifactError::Invalid("invalid PBT checkpoint marker").into());
            }
        } else {
            // A crash may sync the root record before publishing its small checkpoint marker.
            let state = self.database.load::<H>(root, count, self.limits)?;
            self.write_marker(root, count)?;
            return Ok(state);
        }
        Ok(self.database.load::<H>(root, count, self.limits)?)
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

fn cache_state<H>(cache: &mut BTreeMap<B256, StateTree<H>>, root: B256, state: StateTree<H>) {
    // Cache eviction never removes the durable checkpoint needed by a fork.
    if cache.len() >= 64 {
        cache.pop_first();
    }
    cache.insert(root, state);
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
