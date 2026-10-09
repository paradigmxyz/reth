//! Optional observers and deterministic failures over real database operations.

use crate::DatabaseError;
use parking_lot::Mutex;
use std::sync::Arc;

/// Shared test instrumentation scoped to one database environment.
#[derive(Clone, Debug, Default)]
pub struct DatabaseTestHooks(Arc<Mutex<HookState>>);

/// A raw database read, recorded before value decoding.
#[derive(Clone, Debug)]
pub struct DatabaseRead {
    /// Table accessed.
    pub table: String,
    /// Encoded lookup key.
    pub key: Vec<u8>,
    /// Persisted value, if present.
    pub value: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
struct HookState {
    reads: Vec<DatabaseRead>,
    writes: usize,
    fail_write: Option<usize>,
    fail_read: Option<(&'static str, DatabaseError)>,
    failures: usize,
}

impl DatabaseTestHooks {
    /// Copy observations without resetting them.
    pub fn reads(&self) -> Vec<DatabaseRead> {
        self.0.lock().reads.clone()
    }
    /// Start a new observation interval.
    pub fn clear_reads(&self) {
        self.0.lock().reads.clear();
    }
    /// Number of attempted writes.
    pub fn write_count(&self) -> usize {
        self.0.lock().writes
    }
    /// Reset the write counter.
    pub fn reset_write_count(&self) {
        self.0.lock().writes = 0;
    }
    /// Fail the nth subsequent write before mutation.
    pub fn fail_nth_write(&self, index: usize) {
        let mut state = self.0.lock();
        state.writes = 0;
        state.fail_write = Some(index);
    }
    /// Stop injecting write failures.
    pub fn disable_write_failure(&self) {
        self.0.lock().fail_write = None;
    }
    /// Fail the next read of a particular table, preserving its supplied cause.
    pub fn fail_next_table_read(&self, table: &'static str, error: DatabaseError) {
        self.0.lock().fail_read = Some((table, error));
    }
    /// Stop injecting read failures.
    pub fn disable_read_failure(&self) {
        self.0.lock().fail_read = None;
    }
    /// Number of failures actually injected.
    pub fn injected_failures(&self) -> usize {
        self.0.lock().failures
    }
    pub(crate) fn before_read(&self, table: &'static str) -> Result<(), DatabaseError> {
        let mut state = self.0.lock();
        if state.fail_read.as_ref().is_some_and(|(target, _)| *target == table) {
            state.failures += 1;
            return Err(state.fail_read.take().expect("matched failure").1);
        }
        Ok(())
    }
    pub(crate) fn read(&self, table: &'static str, key: &[u8], value: Option<&[u8]>) {
        self.0.lock().reads.push(DatabaseRead {
            table: table.to_owned(),
            key: key.to_vec(),
            value: value.map(<[u8]>::to_vec),
        });
    }
    pub(crate) fn before_write(&self) -> Result<(), DatabaseError> {
        let mut state = self.0.lock();
        state.writes += 1;
        if state.fail_write == Some(state.writes) {
            state.failures += 1;
            return Err(DatabaseError::Other("injected database write failure".into()));
        }
        Ok(())
    }
}
