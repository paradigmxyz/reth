use verkle_trie::{from_to_bytes::{FromBytes, ToBytes}, database::{ReadOnlyHigherDb, WriteOnlyHigherDb, meta::{BranchChild, BranchMeta, StemMeta}}};
use verkle_db::{BareMetalDiskDb, BareMetalKVDb, BatchDB, BatchWriter};

// The purpose of this file is to allows us to implement generic implementation for BatchWriter and BareMetalKVDb
// Anything that implements BatchWriter can be used as a WriteOnlyHigherDb
// Anything that implements BareMetalKVDb can be used as a ReadOnlyHigherDb

pub(crate) const LEAF_TABLE_MARKER: u8 = 0;
pub(crate) const STEM_TABLE_MARKER: u8 = 1;
pub(crate) const BRANCH_TABLE_MARKER: u8 = 2;

// GenericBatchWriter does not write the values to disk
// We need to flush them later on
// This struct allows us to provide default implementations to everything that is a
// BatchWriter
pub struct GenericBatchWriter<T: BatchWriter> {
    pub inner: T,
}

impl<T: BatchWriter> WriteOnlyHigherDb for GenericBatchWriter<T> {
    fn insert_leaf(&mut self, key: [u8; 32], value: [u8; 32], _depth: u8) -> Option<Vec<u8>> {
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(LEAF_TABLE_MARKER);
        labelled_key.extend_from_slice(&key);
        self.inner.batch_put(&labelled_key, &value);
        None
    }

    fn insert_stem(&mut self, key: [u8; 31], meta: StemMeta, _depth: u8) -> Option<StemMeta> {
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(STEM_TABLE_MARKER);
        labelled_key.extend_from_slice(&key);
        self.inner
            .batch_put(&labelled_key, &meta.to_bytes().unwrap());
        None
    }

    fn add_stem_as_branch_child(
        &mut self,
        branch_child_id: Vec<u8>,
        stem_id: [u8; 31],
        _depth: u8,
    ) -> Option<BranchChild> {
        let mut labelled_key = Vec::with_capacity(branch_child_id.len() + 1);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend(branch_child_id);

        self.inner.batch_put(&labelled_key, &stem_id);
        None
    }

    fn insert_branch(&mut self, key: Vec<u8>, meta: BranchMeta, _depth: u8) -> Option<BranchMeta> {
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(&key);

        self.inner
            .batch_put(&labelled_key, &meta.to_bytes().unwrap());
        None
    }
}

// This struct allows us to provide a default implementation of ReadOnlyHigherDB to
// all structs that implement BatchDB

#[derive(Debug)]
pub struct GenericBatchDB<DB> {
    pub inner: DB,
}

impl<DB> GenericBatchDB<DB> {
    
    pub const fn new(inner: DB) -> Self {
        Self { inner }
    }
}

impl<T> std::ops::Deref for GenericBatchDB<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: BatchDB> BatchDB for GenericBatchDB<T> {
    type BatchWrite = T::BatchWrite;

    fn flush(&mut self, batch: Self::BatchWrite) {
        self.inner.flush(batch)
    }
}

impl<T: BareMetalDiskDb> BareMetalDiskDb for GenericBatchDB<T> {
    fn from_path<P: AsRef<std::path::Path>>(path: P) -> Self {
        Self {
            inner: T::from_path(path),
        }
    }

    const DEFAULT_PATH: &'static str = T::DEFAULT_PATH;
}

impl<T: BareMetalKVDb> ReadOnlyHigherDb for GenericBatchDB<T> {
    fn get_leaf(&self, key: [u8; 32]) -> Option<[u8; 32]> {
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(LEAF_TABLE_MARKER);
        labelled_key.extend_from_slice(&key);

        self.inner
            .fetch(&labelled_key)
            .map(|bytes| bytes.try_into().unwrap())
    }

    fn get_stem_meta(&self, stem_key: [u8; 31]) -> Option<StemMeta> {
        let mut labelled_key = Vec::with_capacity(stem_key.len() + 1);
        labelled_key.push(STEM_TABLE_MARKER);
        labelled_key.extend_from_slice(&stem_key);

        self.inner
            .fetch(&labelled_key)
            .map(|old_val_bytes| StemMeta::from_bytes(old_val_bytes).unwrap())
    }

    fn get_branch_children(&self, branch_id: &[u8]) -> Vec<(u8, BranchChild)> {
        let mut children = Vec::with_capacity(256);

        let mut labelled_key = Vec::with_capacity(branch_id.len() + 1);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(branch_id);

        for i in 0u8..=255 {
            let mut child = labelled_key.clone();
            child.push(i);

            let child_value = self.inner.fetch(&child);

            if let Some(x) = child_value {
                children.push((i, BranchChild::from_bytes(x).unwrap()))
            }
        }

        children
    }

    fn get_branch_meta(&self, key: &[u8]) -> Option<BranchMeta> {
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(key);

        self.inner
            .fetch(&labelled_key)
            .map(|old_val_bytes| BranchMeta::from_bytes(old_val_bytes).unwrap())
    }

    fn get_branch_child(&self, branch_id: &[u8], index: u8) -> Option<BranchChild> {
        let mut labelled_key = Vec::with_capacity(branch_id.len() + 2);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(branch_id);
        labelled_key.push(index);
        self.inner
            .fetch(&labelled_key)
            .map(|old_val_bytes| BranchChild::from_bytes(old_val_bytes).unwrap())
    }

    fn get_stem_children(&self, stem_key: [u8; 31]) -> Vec<(u8, [u8; 32])> {
        let mut children = Vec::with_capacity(256);

        let mut labelled_key = Vec::with_capacity(stem_key.len() + 1);
        labelled_key.push(LEAF_TABLE_MARKER);
        labelled_key.extend_from_slice(&stem_key);

        for i in 0u8..=255 {
            let mut child = labelled_key.clone();
            child.push(i);

            let child_value = self.inner.fetch(&child);

            if let Some(x) = child_value {
                children.push((i, x.try_into().unwrap()))
            }
        }

        children
    }
}
