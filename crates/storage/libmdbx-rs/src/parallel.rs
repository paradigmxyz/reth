//! Parallel write preparation for MDBX transactions.
//!
//! This module provides a mechanism to prepare writes for multiple tables in parallel,
//! then apply them atomically through a single transaction. This is useful when you have
//! CPU-bound data preparation that can be parallelized, even though MDBX writes are
//! inherently single-threaded.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Parallel Prep Phase (CPU-bound, multi-threaded)            │
//! │  ├── Thread 1: Prepare Headers writes → WriteSet           │
//! │  ├── Thread 2: Prepare Transactions writes → WriteSet      │
//! │  └── Thread 3: Prepare State writes → WriteSet             │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Sequential Apply Phase (I/O-bound, single-threaded)        │
//! │  └── Apply all WriteSets to single RW transaction          │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use crate::{error::Result, flags::WriteFlags, CommitLatency, Environment, Transaction, RW};

/// A pending write operation.
#[derive(Debug)]
pub enum WriteOp {
    /// Insert or update a key-value pair.
    Put { key: Vec<u8>, value: Vec<u8>, flags: ffi::MDBX_env_flags_t },
    /// Delete a key (and optionally a specific value for DUPSORT tables).
    Delete { key: Vec<u8>, value: Option<Vec<u8>> },
}

/// A set of write operations for a single table, prepared in parallel.
///
/// Operations are stored in a `BTreeMap` keyed by the operation key to ensure
/// deterministic ordering and allow for deduplication of operations on the same key.
#[derive(Debug, Default)]
pub struct WriteSet {
    /// Table name (None for the default/unnamed table).
    table_name: Option<String>,
    /// Pending operations, keyed by the operation key for ordering.
    operations: Vec<WriteOp>,
}

impl WriteSet {
    /// Creates a new empty `WriteSet` for the given table.
    pub fn new(table_name: Option<&str>) -> Self {
        Self { table_name: table_name.map(String::from), operations: Vec::new() }
    }

    /// Creates a new `WriteSet` with pre-allocated capacity.
    pub fn with_capacity(table_name: Option<&str>, capacity: usize) -> Self {
        Self { table_name: table_name.map(String::from), operations: Vec::with_capacity(capacity) }
    }

    /// Adds a put operation to the write set.
    pub fn put(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.put_with_flags(key, value, WriteFlags::empty());
    }

    /// Adds a put operation with custom flags to the write set.
    pub fn put_with_flags(
        &mut self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
        flags: WriteFlags,
    ) {
        self.operations.push(WriteOp::Put {
            key: key.into(),
            value: value.into(),
            flags: flags.bits(),
        });
    }

    /// Adds a delete operation to the write set.
    pub fn delete(&mut self, key: impl Into<Vec<u8>>) {
        self.operations.push(WriteOp::Delete { key: key.into(), value: None });
    }

    /// Adds a delete operation with a specific value (for DUPSORT tables).
    pub fn delete_value(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.operations.push(WriteOp::Delete { key: key.into(), value: Some(value.into()) });
    }

    /// Returns the number of operations in this write set.
    pub const fn len(&self) -> usize {
        self.operations.len()
    }

    /// Returns true if this write set has no operations.
    pub const fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Returns the table name for this write set.
    pub fn table_name(&self) -> Option<&str> {
        self.table_name.as_deref()
    }
}

/// A batch of write sets to be applied atomically.
///
/// This allows collecting writes from multiple parallel preparation threads
/// and applying them all in a single transaction.
#[derive(Debug, Default)]
pub struct ParallelWriteBatch {
    write_sets: Vec<WriteSet>,
}

impl ParallelWriteBatch {
    /// Creates a new empty batch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new batch with pre-allocated capacity for the expected number of tables.
    pub fn with_capacity(num_tables: usize) -> Self {
        Self { write_sets: Vec::with_capacity(num_tables) }
    }

    /// Adds a write set to the batch.
    ///
    /// This is typically called after parallel preparation is complete,
    /// collecting results from multiple threads.
    pub fn add_write_set(&mut self, write_set: WriteSet) {
        self.write_sets.push(write_set);
    }

    /// Merges another batch into this one.
    pub fn merge(&mut self, other: Self) {
        self.write_sets.extend(other.write_sets);
    }

    /// Returns the total number of operations across all write sets.
    pub fn total_operations(&self) -> usize {
        self.write_sets.iter().map(|ws| ws.len()).sum()
    }

    /// Returns the number of write sets in this batch.
    pub const fn num_write_sets(&self) -> usize {
        self.write_sets.len()
    }

    /// Applies all write sets to the given environment atomically.
    ///
    /// Opens a single RW transaction, applies all operations from all write sets,
    /// and commits. If any operation fails, the entire transaction is aborted.
    pub fn apply(self, env: &Environment) -> Result<CommitLatency> {
        let txn = env.begin_rw_txn()?;
        self.apply_to_txn(&txn)?;
        txn.commit()
    }

    /// Applies all write sets to an existing RW transaction.
    ///
    /// This allows the caller to do additional work in the same transaction
    /// before committing.
    pub fn apply_to_txn(self, txn: &Transaction<RW>) -> Result<()> {
        for write_set in self.write_sets {
            let db = match &write_set.table_name {
                Some(name) => txn.open_db(Some(name))?,
                None => txn.open_db(None)?,
            };
            let dbi = db.dbi();

            for op in write_set.operations {
                match op {
                    WriteOp::Put { key, value, flags } => {
                        let write_flags = WriteFlags::from_bits_truncate(flags);
                        txn.put(dbi, &key, &value, write_flags)?;
                    }
                    WriteOp::Delete { key, value } => {
                        txn.del(dbi, &key, value.as_deref())?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Builder for parallel write preparation.
///
/// Provides a convenient API for spawning parallel preparation tasks
/// and collecting their results.
#[derive(Debug)]
pub struct ParallelWriteBuilder<'env> {
    env: &'env Environment,
}

impl<'env> ParallelWriteBuilder<'env> {
    /// Creates a new parallel write builder for the given environment.
    pub const fn new(env: &'env Environment) -> Self {
        Self { env }
    }

    /// Returns the environment this builder is associated with.
    pub const fn env(&self) -> &Environment {
        self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatabaseFlags;
    use std::{
        sync::{Arc, Barrier},
        thread,
    };
    use tempfile::tempdir;

    #[test]
    fn test_write_set_basic() {
        let mut ws = WriteSet::new(None);
        assert!(ws.is_empty());

        ws.put(b"key1", b"val1");
        ws.put(b"key2", b"val2");
        ws.delete(b"key3");

        assert_eq!(ws.len(), 3);
        assert!(!ws.is_empty());
    }

    #[test]
    fn test_write_set_with_capacity() {
        let ws = WriteSet::with_capacity(Some("test_table"), 100);
        assert_eq!(ws.table_name(), Some("test_table"));
        assert!(ws.is_empty());
    }

    #[test]
    fn test_parallel_batch_apply() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().set_max_dbs(10).open(dir.path()).unwrap();

        // Create tables first
        {
            let txn = env.begin_rw_txn().unwrap();
            txn.create_db(Some("table1"), DatabaseFlags::empty()).unwrap();
            txn.create_db(Some("table2"), DatabaseFlags::empty()).unwrap();
            txn.commit().unwrap();
        }

        // Prepare writes in parallel
        let mut batch = ParallelWriteBatch::new();

        let barrier = Arc::new(Barrier::new(2));
        let barrier1 = barrier.clone();
        let barrier2 = barrier.clone();

        let handle1 = thread::spawn(move || {
            let mut ws = WriteSet::new(Some("table1"));
            barrier1.wait(); // Ensure both threads start together
            for i in 0..100 {
                ws.put(format!("key{i}"), format!("val1_{i}"));
            }
            ws
        });

        let handle2 = thread::spawn(move || {
            let mut ws = WriteSet::new(Some("table2"));
            barrier2.wait(); // Ensure both threads start together
            for i in 0..100 {
                ws.put(format!("key{i}"), format!("val2_{i}"));
            }
            ws
        });

        batch.add_write_set(handle1.join().unwrap());
        batch.add_write_set(handle2.join().unwrap());

        assert_eq!(batch.total_operations(), 200);
        assert_eq!(batch.num_write_sets(), 2);

        // Apply atomically
        batch.apply(&env).unwrap();

        // Verify writes
        let txn = env.begin_ro_txn().unwrap();

        let db1 = txn.open_db(Some("table1")).unwrap();
        let db2 = txn.open_db(Some("table2")).unwrap();

        for i in 0..100 {
            let key = format!("key{i}");
            let val1: Vec<u8> = txn.get(db1.dbi(), key.as_bytes()).unwrap().unwrap();
            let val2: Vec<u8> = txn.get(db2.dbi(), key.as_bytes()).unwrap().unwrap();

            assert_eq!(val1, format!("val1_{i}").as_bytes());
            assert_eq!(val2, format!("val2_{i}").as_bytes());
        }
    }

    #[test]
    fn test_parallel_batch_with_deletes() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        // Pre-populate with data
        {
            let txn = env.begin_rw_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            for i in 0..10 {
                txn.put(db.dbi(), format!("key{i}"), format!("old_val{i}"), WriteFlags::empty())
                    .unwrap();
            }
            txn.commit().unwrap();
        }

        // Prepare mixed operations
        let mut ws = WriteSet::new(None);
        ws.put(b"key0", b"new_val0"); // Update
        ws.put(b"key100", b"new_val100"); // Insert
        ws.delete(b"key5"); // Delete

        let mut batch = ParallelWriteBatch::new();
        batch.add_write_set(ws);
        batch.apply(&env).unwrap();

        // Verify
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();

        let val0: Vec<u8> = txn.get(db.dbi(), b"key0").unwrap().unwrap();
        assert_eq!(val0, b"new_val0");

        let val100: Vec<u8> = txn.get(db.dbi(), b"key100").unwrap().unwrap();
        assert_eq!(val100, b"new_val100");

        let val5: Option<Vec<u8>> = txn.get(db.dbi(), b"key5").unwrap();
        assert!(val5.is_none());

        // Unchanged keys should still have old values
        let val1: Vec<u8> = txn.get(db.dbi(), b"key1").unwrap().unwrap();
        assert_eq!(val1, b"old_val1");
    }

    #[test]
    fn test_parallel_batch_apply_to_existing_txn() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        let mut ws = WriteSet::new(None);
        ws.put(b"batch_key", b"batch_val");

        let mut batch = ParallelWriteBatch::new();
        batch.add_write_set(ws);

        // Apply to existing transaction with additional work
        let txn = env.begin_rw_txn().unwrap();
        let db = txn.open_db(None).unwrap();

        // Do some work before applying batch
        txn.put(db.dbi(), b"txn_key", b"txn_val", WriteFlags::empty()).unwrap();

        // Apply batch
        batch.apply_to_txn(&txn).unwrap();

        // Do some work after applying batch
        txn.put(db.dbi(), b"txn_key2", b"txn_val2", WriteFlags::empty()).unwrap();

        txn.commit().unwrap();

        // Verify all writes
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();

        assert_eq!(txn.get::<Vec<u8>>(db.dbi(), b"batch_key").unwrap().unwrap(), b"batch_val");
        assert_eq!(txn.get::<Vec<u8>>(db.dbi(), b"txn_key").unwrap().unwrap(), b"txn_val");
        assert_eq!(txn.get::<Vec<u8>>(db.dbi(), b"txn_key2").unwrap().unwrap(), b"txn_val2");
    }

    #[test]
    fn test_parallel_batch_merge() {
        let mut batch1 = ParallelWriteBatch::new();
        let mut ws1 = WriteSet::new(Some("table1"));
        ws1.put(b"k1", b"v1");
        batch1.add_write_set(ws1);

        let mut batch2 = ParallelWriteBatch::new();
        let mut ws2 = WriteSet::new(Some("table2"));
        ws2.put(b"k2", b"v2");
        batch2.add_write_set(ws2);

        batch1.merge(batch2);

        assert_eq!(batch1.num_write_sets(), 2);
        assert_eq!(batch1.total_operations(), 2);
    }

    #[test]
    fn test_high_volume_parallel_writes() {
        let dir = tempdir().unwrap();
        let env = Environment::builder()
            .set_max_dbs(10)
            .set_geometry(crate::Geometry {
                size: Some(0..=1024 * 1024 * 1024), // 1GB max
                ..Default::default()
            })
            .open(dir.path())
            .unwrap();

        // Create tables
        {
            let txn = env.begin_rw_txn().unwrap();
            for i in 0..3 {
                txn.create_db(Some(&format!("table{i}")), DatabaseFlags::empty()).unwrap();
            }
            txn.commit().unwrap();
        }

        const OPS_PER_TABLE: usize = 10_000;

        // Spawn threads to prepare writes
        let handles: Vec<_> = (0..3)
            .map(|table_idx| {
                thread::spawn(move || {
                    let mut ws =
                        WriteSet::with_capacity(Some(&format!("table{table_idx}")), OPS_PER_TABLE);
                    for i in 0..OPS_PER_TABLE {
                        let key = format!("{table_idx}_{i:08}");
                        let value = vec![table_idx as u8; 100]; // 100 byte values
                        ws.put(key, value);
                    }
                    ws
                })
            })
            .collect();

        let mut batch = ParallelWriteBatch::with_capacity(3);
        for handle in handles {
            batch.add_write_set(handle.join().unwrap());
        }

        assert_eq!(batch.total_operations(), OPS_PER_TABLE * 3);

        // Apply and measure
        let start = std::time::Instant::now();
        let latency = batch.apply(&env).unwrap();
        let elapsed = start.elapsed();

        println!(
            "Applied {} ops in {:?} (commit: {:?})",
            OPS_PER_TABLE * 3,
            elapsed,
            latency.whole()
        );

        // Verify a sample
        let txn = env.begin_ro_txn().unwrap();
        for table_idx in 0..3 {
            let db = txn.open_db(Some(&format!("table{table_idx}"))).unwrap();
            let key = format!("{table_idx}_{:08}", OPS_PER_TABLE / 2);
            let val: Vec<u8> = txn.get(db.dbi(), key.as_bytes()).unwrap().unwrap();
            assert_eq!(val.len(), 100);
            assert!(val.iter().all(|&b| b == table_idx as u8));
        }
    }
}
