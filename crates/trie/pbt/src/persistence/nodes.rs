//! Content-addressed trie nodes shared by durable fork checkpoints.

use super::StoreError;
use crate::{
    hash::{branch_hash, leaf_hash},
    key::{self, ACCOUNT_KEY_LENGTH, CODE_HASH_LEAF_KEY, DELEGATION_LEAF_KEY, STORAGE_KEY_LENGTH},
    migration::{restore_leaves, ArtifactError, DecodeLimits, Reader},
    tree::Node,
    StateTree, TreeHasher,
};
use alloy_primitives::{Address, B256, U256};
use rocksdb::{Options, WriteBatch, WriteOptions, DB};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// The database lock and writer serialization are shared across handles to the same directory.
#[derive(Debug)]
pub(super) struct NodeDatabase {
    db: DB,
    pub(super) mutation: Mutex<()>,
}

impl NodeDatabase {
    pub(super) fn open(path: &Path) -> Result<Arc<Self>, StoreError> {
        // RocksDB owns an exclusive file lock. Reopening a store in this process must share it.
        static DATABASES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<NodeDatabase>>>> = OnceLock::new();
        let path = path
            .canonicalize()
            .map_err(|error| reth_fs_util::FsPathError::metadata(error, path))?;
        let mut databases =
            DATABASES.get_or_init(Mutex::default).lock().map_err(|_| StoreError::Poisoned)?;
        databases.retain(|_, database| database.strong_count() != 0);
        if let Some(database) = databases.get(&path).and_then(Weak::upgrade) {
            return Ok(database);
        }
        let mut options = Options::default();
        options.create_if_missing(true);
        let db = DB::open(&options, path.join("nodes"))?;
        match db.get(b"schema")? {
            Some(version) if version.as_slice() == STORE_VERSION.to_be_bytes() => {}
            Some(_) => {
                return Err(ArtifactError::Invalid("unsupported PBT database version").into())
            }
            None => {
                let mut batch = WriteBatch::default();
                batch.put(b"schema", STORE_VERSION.to_be_bytes());
                write(&db, batch)?;
            }
        }
        let database = Arc::new(Self { db, mutation: Mutex::default() });
        databases.insert(path, Arc::downgrade(&database));
        Ok(database)
    }

    pub(super) fn leaf_count(&self, root: B256) -> Result<Option<usize>, StoreError> {
        self.db
            .get(database_key(ROOT, root))?
            .map(|value| {
                let count = value
                    .as_slice()
                    .try_into()
                    .map_err(|_| ArtifactError::Invalid("invalid PBT root record"))?;
                usize::try_from(u64::from_be_bytes(count))
                    .map_err(|_| ArtifactError::ResourceLimit.into())
            })
            .transpose()
    }

    /// Callers hold `mutation` until the checkpoint reference has also been published.
    pub(super) fn save<H: TreeHasher>(
        &self,
        state: &StateTree<H>,
        limits: DecodeLimits,
    ) -> Result<(), StoreError> {
        if state.account_count() > limits.max_accounts || state.tree().len() > limits.max_leaves {
            return Err(ArtifactError::ResourceLimit.into());
        }
        if state.accounts().any(|(_, account)| account.is_empty()) {
            return Err(ArtifactError::Invalid("empty anchor account").into());
        }
        let mut total_code = 0usize;
        for size in state.code_sizes() {
            total_code = total_code.checked_add(size).ok_or(ArtifactError::ResourceLimit)?;
            if size > limits.max_code_size || total_code > limits.max_total_code_size {
                return Err(ArtifactError::ResourceLimit.into());
            }
        }
        let mut stack: Vec<_> =
            state.tree().root.as_deref().map(|node| (node, false)).into_iter().collect();
        let mut batch = WriteBatch::default();
        while let Some((node, expanded)) = stack.pop() {
            let hash = node.hash();
            if self.db.get_pinned(database_key(NODE, hash))?.is_some() {
                continue;
            }
            // Publish children before parents so a cancelled multi-batch save can be retried.
            if !expanded && let Node::Branch { left, right, .. } = node {
                stack.push((node, true));
                stack.push((right, false));
                stack.push((left, false));
                continue;
            }
            let mut encoded = Vec::new();
            match node {
                Node::Leaf { key, value, .. } => {
                    encoded.push(LEAF);
                    encoded.extend_from_slice(
                        &u16::try_from(key.len())
                            .map_err(|_| ArtifactError::ResourceLimit)?
                            .to_be_bytes(),
                    );
                    encoded.extend_from_slice(key);
                    encoded.extend_from_slice(value.as_slice());
                    if key[0] == key::ACCOUNT_ZONE &&
                        matches!(key[33], CODE_HASH_LEAF_KEY | DELEGATION_LEAF_KEY)
                    {
                        let address = state
                            .address_preimage(B256::from_slice(&key[1..33]))
                            .ok_or(ArtifactError::Invalid("missing account preimage"))?;
                        encoded.extend_from_slice(address.as_slice());
                    } else if key[0] == key::STORAGE_ZONE {
                        let address = state
                            .address_preimage(B256::from_slice(&key[1..33]))
                            .ok_or(ArtifactError::Invalid("missing account preimage"))?;
                        let slot = state
                            .storage_preimage(address, key)
                            .ok_or(ArtifactError::Invalid("missing storage preimage"))?;
                        encoded.extend_from_slice(&slot.to_be_bytes::<32>());
                    }
                }
                Node::Branch { prefix, left, right, .. } => {
                    encoded.push(BRANCH);
                    encoded.extend_from_slice(
                        &u16::try_from(prefix.len())
                            .map_err(|_| ArtifactError::ResourceLimit)?
                            .to_be_bytes(),
                    );
                    let start = encoded.len();
                    encoded.resize(start + prefix.len().div_ceil(8), 0);
                    for (index, bit) in prefix.iter().enumerate() {
                        encoded[start + index / 8] |= u8::from(*bit) << (7 - index % 8);
                    }
                    encoded.extend_from_slice(left.hash().as_slice());
                    encoded.extend_from_slice(right.hash().as_slice());
                }
            }
            batch.put(database_key(NODE, hash), encoded);
            if batch.size_in_bytes() >= WRITE_BATCH_BYTES {
                write(&self.db, batch)?;
                batch = WriteBatch::default();
            }
        }
        // All batches, including this root record, reach the WAL before a reference is published.
        batch.put(
            database_key(ROOT, state.root()),
            u64::try_from(state.tree().len())
                .map_err(|_| ArtifactError::ResourceLimit)?
                .to_be_bytes(),
        );
        write(&self.db, batch)
    }

    pub(super) fn load<H: TreeHasher>(
        &self,
        root: B256,
        count: usize,
        limits: DecodeLimits,
    ) -> Result<StateTree<H>, StoreError> {
        if count > limits.max_leaves {
            return Err(ArtifactError::ResourceLimit.into());
        }
        let mut leaves = BTreeMap::new();
        let mut addresses = BTreeMap::new();
        let mut slots = BTreeMap::new();
        let mut stack = Vec::new();
        if !root.is_zero() {
            stack.push((root, 0usize));
        }
        let mut visited = 0usize;
        while let Some((hash, depth)) = stack.pop() {
            visited = visited.checked_add(1).ok_or(ArtifactError::ResourceLimit)?;
            if visited > count.saturating_mul(2) || depth > STORAGE_KEY_LENGTH * 8 {
                return Err(ArtifactError::ResourceLimit.into());
            }
            match self.node::<H>(hash)? {
                StoredNode::Leaf { key, value, preimage } => {
                    if leaves.len() >= count || leaves.insert(key.clone(), value).is_some() {
                        return Err(ArtifactError::Invalid("duplicate or excess PBT leaf").into());
                    }
                    if let Some(preimage) = preimage {
                        if key[0] == key::ACCOUNT_ZONE {
                            addresses.insert(
                                B256::from_slice(&key[1..33]),
                                Address::from_slice(&preimage),
                            );
                            if addresses.len() > limits.max_accounts {
                                return Err(ArtifactError::ResourceLimit.into());
                            }
                        } else {
                            slots.insert(key, U256::from_be_slice(&preimage));
                        }
                    }
                }
                StoredNode::Branch { bits, left, right } => {
                    let depth = depth.checked_add(bits + 1).ok_or(ArtifactError::ResourceLimit)?;
                    stack.push((right, depth));
                    stack.push((left, depth));
                }
            }
        }
        if leaves.len() != count {
            return Err(ArtifactError::Invalid("PBT leaf count mismatch").into());
        }
        Ok(restore_leaves::<H>(leaves, addresses, slots, root, limits)?)
    }

    fn node<H: TreeHasher>(&self, hash: B256) -> Result<StoredNode, StoreError> {
        let value = self
            .db
            .get_pinned(database_key(NODE, hash))?
            .ok_or(ArtifactError::Invalid("missing PBT node"))?;
        if value.len() > MAX_NODE_BYTES {
            return Err(ArtifactError::ResourceLimit.into());
        }
        let mut reader = Reader(&value);
        let tag = reader.take(1)?[0];
        let length = usize::from(u16::from_be_bytes(reader.array()?));
        let node = match tag {
            LEAF => {
                if !matches!(length, ACCOUNT_KEY_LENGTH | STORAGE_KEY_LENGTH) {
                    return Err(ArtifactError::Invalid("invalid PBT key length").into());
                }
                let key = reader.take(length)?.to_vec();
                if !matches!(
                    (key[0], length),
                    (key::ACCOUNT_ZONE | key::CODE_ZONE, ACCOUNT_KEY_LENGTH) |
                        (key::STORAGE_ZONE, STORAGE_KEY_LENGTH)
                ) {
                    return Err(ArtifactError::Invalid("invalid PBT key zone").into());
                }
                let value = B256::from_slice(reader.take(32)?);
                if value.is_zero() || leaf_hash::<H>(&key, value) != hash {
                    return Err(ArtifactError::Invalid("invalid PBT leaf hash").into());
                }
                let preimage = if key[0] == key::ACCOUNT_ZONE &&
                    matches!(key[33], CODE_HASH_LEAF_KEY | DELEGATION_LEAF_KEY)
                {
                    Some(reader.take(20)?.to_vec())
                } else if key[0] == key::STORAGE_ZONE {
                    Some(reader.take(32)?.to_vec())
                } else {
                    None
                };
                StoredNode::Leaf { key, value, preimage }
            }
            BRANCH => {
                if length >= STORAGE_KEY_LENGTH * 8 {
                    return Err(ArtifactError::Invalid("invalid PBT branch length").into());
                }
                let packed = reader.take(length.div_ceil(8))?;
                if length % 8 != 0 &&
                    packed.last().is_some_and(|last| *last & ((1 << (8 - length % 8)) - 1) != 0)
                {
                    return Err(ArtifactError::Invalid("noncanonical PBT prefix padding").into());
                }
                let prefix: Vec<_> = (0..length)
                    .map(|index| packed[index / 8] & (1 << (7 - index % 8)) != 0)
                    .collect();
                let left = B256::from_slice(reader.take(32)?);
                let right = B256::from_slice(reader.take(32)?);
                if left.is_zero() ||
                    right.is_zero() ||
                    branch_hash::<H>(&prefix, left, right) != hash
                {
                    return Err(ArtifactError::Invalid("invalid PBT branch hash").into());
                }
                StoredNode::Branch { bits: length, left, right }
            }
            _ => return Err(ArtifactError::Invalid("invalid PBT node tag").into()),
        };
        if !reader.0.is_empty() {
            return Err(ArtifactError::Invalid("trailing PBT node bytes").into());
        }
        Ok(node)
    }

    pub(super) fn remove_root(&self, root: B256) -> Result<(), StoreError> {
        let mut batch = WriteBatch::default();
        batch.delete(database_key(ROOT, root));
        write(&self.db, batch)
    }

    /// Marks every registered root before sweeping nodes. Interrupted marking deletes nothing.
    pub(super) fn collect<H: TreeHasher>(&self) -> Result<(), StoreError> {
        self.delete_prefix(MARK)?;
        for root in self.hashes(ROOT) {
            let root = root?;
            let mut stack = Vec::new();
            if !root.is_zero() {
                stack.push((root, 0usize));
            }
            let mut batch = WriteBatch::default();
            while let Some((hash, depth)) = stack.pop() {
                if depth > STORAGE_KEY_LENGTH * 8 {
                    return Err(ArtifactError::Invalid("invalid PBT node depth").into());
                }
                if self.db.get_pinned(database_key(MARK, hash))?.is_some() {
                    continue;
                }
                if let StoredNode::Branch { bits, left, right } = self.node::<H>(hash)? {
                    stack.push((right, depth + bits + 1));
                    stack.push((left, depth + bits + 1));
                }
                batch.put(database_key(MARK, hash), []);
                if batch.size_in_bytes() >= WRITE_BATCH_BYTES {
                    write(&self.db, batch)?;
                    batch = WriteBatch::default();
                }
            }
            write(&self.db, batch)?;
        }
        let mut batch = WriteBatch::default();
        for hash in self.hashes(NODE) {
            let hash = hash?;
            if self.db.get_pinned(database_key(MARK, hash))?.is_none() {
                batch.delete(database_key(NODE, hash));
            }
            if batch.size_in_bytes() >= WRITE_BATCH_BYTES {
                write(&self.db, batch)?;
                batch = WriteBatch::default();
            }
        }
        write(&self.db, batch)?;
        self.delete_prefix(MARK)
    }

    fn delete_prefix(&self, prefix: u8) -> Result<(), StoreError> {
        let mut batch = WriteBatch::default();
        for hash in self.hashes(prefix) {
            batch.delete(database_key(prefix, hash?));
            if batch.size_in_bytes() >= WRITE_BATCH_BYTES {
                write(&self.db, batch)?;
                batch = WriteBatch::default();
            }
        }
        write(&self.db, batch)
    }

    pub(super) fn has_roots(&self) -> Result<bool, StoreError> {
        Ok(self.hashes(ROOT).next().transpose()?.is_some())
    }

    fn hashes(&self, prefix: u8) -> impl Iterator<Item = Result<B256, StoreError>> + '_ {
        // Key-only iteration avoids allocating corrupt, potentially oversized node values in GC.
        let mut iterator = self.db.raw_iterator();
        iterator.seek([prefix]);
        let mut finished = false;
        std::iter::from_fn(move || {
            if finished {
                return None;
            }
            let Some(key) = iterator.key() else {
                finished = true;
                return iterator.status().err().map(|error| Err(error.into()));
            };
            if key.first() != Some(&prefix) {
                finished = true;
                return None;
            }
            let hash = if key.len() == 33 {
                Ok(B256::from_slice(&key[1..]))
            } else {
                Err(ArtifactError::Invalid("invalid PBT database key").into())
            };
            iterator.next();
            Some(hash)
        })
    }
}

enum StoredNode {
    Leaf { key: Vec<u8>, value: B256, preimage: Option<Vec<u8>> },
    Branch { bits: usize, left: B256, right: B256 },
}

pub(super) const STORE_VERSION: u32 = 2;
const NODE: u8 = b'n';
const ROOT: u8 = b'r';
const MARK: u8 = b'g';
const LEAF: u8 = 0;
const BRANCH: u8 = 1;
const WRITE_BATCH_BYTES: usize = 8 * 1024 * 1024;
const MAX_NODE_BYTES: usize = 3 + STORAGE_KEY_LENGTH + 64;

fn database_key(prefix: u8, hash: B256) -> [u8; 33] {
    let mut key = [prefix; 33];
    key[1..].copy_from_slice(hash.as_slice());
    key
}

fn write(db: &DB, batch: WriteBatch) -> Result<(), StoreError> {
    let mut options = WriteOptions::default();
    options.set_sync(true);
    Ok(db.write_opt(batch, &options)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migration::Artifacts, AccountData, Blake3Hasher};
    use alloy_primitives::Bytes;

    #[test]
    fn checkpoints_write_only_changed_paths_and_collect_unshared_nodes() {
        let directory = tempfile::tempdir().unwrap();
        let database = NodeDatabase::open(directory.path()).unwrap();
        assert!(Arc::ptr_eq(&database, &NodeDatabase::open(directory.path()).unwrap()));
        let limits = DecodeLimits::default();
        let mut state = StateTree::<Blake3Hasher>::new();
        state.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() }).unwrap();
        for slot in 0..1024 {
            state.set_storage(Address::ZERO, U256::from(slot), B256::repeat_byte(1)).unwrap();
        }
        database.save(&state, limits).unwrap();
        let first_root = state.root();
        let first_count = node_count(&database);
        let sequence = database.db.latest_sequence_number();
        database.save(&state, limits).unwrap();
        assert_eq!(node_count(&database), first_count);
        assert_eq!(database.db.latest_sequence_number() - sequence, 1);
        let parent = state.clone();
        state.set_storage(Address::ZERO, U256::from(1023), B256::repeat_byte(2)).unwrap();
        let sequence = database.db.latest_sequence_number();
        database.save(&state, limits).unwrap();
        let added = node_count(&database) - first_count;
        assert!(added > 1 && added <= STORAGE_KEY_LENGTH * 8 + 1, "added {added} nodes");
        assert_eq!(
            database.db.latest_sequence_number() - sequence,
            u64::try_from(added + 1).unwrap()
        );
        assert_eq!(parent.storage(Address::ZERO, U256::from(1023)), B256::repeat_byte(1));
        assert_eq!(
            database.load::<Blake3Hasher>(first_root, parent.tree().len(), limits).unwrap().root(),
            first_root
        );
        database.remove_root(first_root).unwrap();
        database.collect::<Blake3Hasher>().unwrap();
        assert_eq!(node_count(&database), state.tree().len() * 2 - 1);
        assert_eq!(
            database.load::<Blake3Hasher>(state.root(), state.tree().len(), limits).unwrap().root(),
            state.root()
        );
        // Interrupted marking leaves hints that the next collection must rebuild.
        database.db.put(database_key(MARK, first_root), []).unwrap();
        database.collect::<Blake3Hasher>().unwrap();
        assert_eq!(node_count(&database), state.tree().len() * 2 - 1);
    }

    #[test]
    fn cold_load_verifies_code_delegations_preimages_and_node_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = StateTree::<Blake3Hasher>::new();
        for byte in 1..=3 {
            let mut code = if byte == 3 { vec![0xef, 1, 0] } else { vec![0x7f; 95] };
            if byte == 3 {
                code.extend_from_slice(Address::ZERO.as_slice());
            }
            let address = Address::repeat_byte(byte);
            state
                .set_account(
                    address,
                    AccountData { nonce: 1, code: Bytes::from(code), ..Default::default() },
                )
                .unwrap();
            state.set_storage(address, U256::from(63), B256::repeat_byte(5)).unwrap();
            state.set_storage(address, U256::MAX, B256::repeat_byte(6)).unwrap();
        }
        let limits = DecodeLimits::default();
        let database = NodeDatabase::open(directory.path()).unwrap();
        database.save(&state, limits).unwrap();
        drop(database);
        let database = NodeDatabase::open(directory.path()).unwrap();
        let restored =
            database.load::<Blake3Hasher>(state.root(), state.tree().len(), limits).unwrap();
        assert_eq!(
            Artifacts::from_state(&restored).unwrap(),
            Artifacts::from_state(&state).unwrap()
        );
        let (key, _) = state.tree().entries().find(|(key, _)| key[0] == key::STORAGE_ZONE).unwrap();
        let hash = leaf_hash::<Blake3Hasher>(key, state.tree().get(key).unwrap());
        let record = database_key(NODE, hash);
        let original = database.db.get(record).unwrap().unwrap();
        let mut corrupted = original.clone();
        *corrupted.last_mut().unwrap() ^= 1;
        database.db.put(record, &corrupted).unwrap();
        assert!(matches!(
            database.load::<Blake3Hasher>(state.root(), state.tree().len(), limits),
            Err(StoreError::Artifact(ArtifactError::Invalid("invalid storage preimage")))
        ));
        database.db.put(record, original).unwrap();
        database.db.delete(record).unwrap();
        assert!(matches!(
            database.load::<Blake3Hasher>(state.root(), state.tree().len(), limits),
            Err(StoreError::Artifact(ArtifactError::Invalid("missing PBT node")))
        ));
    }

    #[test]
    fn schema_and_empty_account_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let database = NodeDatabase::open(directory.path()).unwrap();
        let mut state = StateTree::<Blake3Hasher>::new();
        state.set_account(Address::ZERO, AccountData::default()).unwrap();
        assert!(matches!(
            database.save(&state, DecodeLimits::default()),
            Err(StoreError::Artifact(ArtifactError::Invalid("empty anchor account")))
        ));
        database.db.put(b"schema", 0u32.to_be_bytes()).unwrap();
        drop(database);
        assert!(matches!(
            NodeDatabase::open(directory.path()),
            Err(StoreError::Artifact(ArtifactError::Invalid("unsupported PBT database version")))
        ));
    }

    fn node_count(database: &NodeDatabase) -> usize {
        database.hashes(NODE).map(Result::unwrap).count()
    }
}
