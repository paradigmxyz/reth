//! Opt-in, transaction-local operation accounting for persistence diagnostics.

use reth_primitives_traits::FastInstant;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
};

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RETH_PERSISTENCE_TIMINGS").is_ok_and(|value| value == "1"));

#[derive(Debug, Default)]
pub(super) struct PersistenceTiming {
    block: AtomicU64,
    state_block: AtomicU64,
    active: AtomicBool,
    tables: Mutex<BTreeMap<&'static str, Arc<TableTiming>>>,
}

impl PersistenceTiming {
    pub(super) fn begin(&self, block: u64, state_block: u64) {
        self.block.store(block, Ordering::Relaxed);
        self.state_block.store(state_block, Ordering::Relaxed);
        if *ENABLED {
            self.tables.lock().unwrap().clear();
            self.active.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn table(&self, name: &'static str) -> Option<Arc<TableTiming>> {
        self.active
            .load(Ordering::Relaxed)
            .then(|| self.tables.lock().unwrap().entry(name).or_default().clone())
    }

    pub(super) fn frontiers(&self) -> (u64, u64) {
        (self.block.load(Ordering::Relaxed), self.state_block.load(Ordering::Relaxed))
    }

    pub(super) fn end(&self) -> Vec<(&'static str, u64, u64)> {
        if !self.active.swap(false, Ordering::Relaxed) {
            return Vec::new()
        }
        std::mem::take(&mut *self.tables.lock().unwrap())
            .into_iter()
            .map(|(table, timing)| {
                (
                    table,
                    timing.operations.load(Ordering::Relaxed),
                    timing.nanos.load(Ordering::Relaxed),
                )
            })
            .collect()
    }
}

#[derive(Debug, Default)]
pub(super) struct TableTiming {
    operations: AtomicU64,
    nanos: AtomicU64,
}

impl TableTiming {
    pub(super) fn record(&self, started: FastInstant) {
        self.nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.operations.fetch_add(1, Ordering::Relaxed);
    }
}

/// Accumulate locally so parallel shard workers do not contend on timing counters.
#[derive(Debug)]
pub(super) struct CursorTiming {
    table: Arc<TableTiming>,
    operations: u64,
    nanos: u64,
}

impl CursorTiming {
    pub(super) const fn new(table: Arc<TableTiming>) -> Self {
        Self { table, operations: 0, nanos: 0 }
    }

    pub(super) fn record(&mut self, started: FastInstant) {
        self.nanos += started.elapsed().as_nanos() as u64;
        self.operations += 1;
    }
}

impl Drop for CursorTiming {
    fn drop(&mut self) {
        self.table.operations.fetch_add(self.operations, Ordering::Relaxed);
        self.table.nanos.fetch_add(self.nanos, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_counters_are_flushed_once_on_drop() {
        let table = Arc::new(TableTiming::default());
        {
            let mut first = CursorTiming::new(table.clone());
            let mut second = CursorTiming::new(table.clone());
            first.record(FastInstant::now());
            first.record(FastInstant::now());
            second.record(FastInstant::now());
            assert_eq!(table.operations.load(Ordering::Relaxed), 0);
        }
        assert_eq!(table.operations.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn accounting_is_transaction_local_and_resets_between_batches() {
        let first = PersistenceTiming::default();
        let second = PersistenceTiming::default();
        assert!(first.table("Accounts").is_none());
        first.active.store(true, Ordering::Relaxed);
        second.active.store(true, Ordering::Relaxed);
        let accounts = first.table("Accounts").unwrap();
        let storage = first.table("Storage").unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| accounts.record(FastInstant::now()));
            scope.spawn(|| storage.record(FastInstant::now()));
        });
        let result = first.end();
        assert_eq!(
            result.iter().map(|(name, count, _)| (*name, *count)).collect::<Vec<_>>(),
            vec![("Accounts", 1), ("Storage", 1)]
        );
        assert!(second.end().is_empty());
        assert!(first.table("Accounts").is_none());
        first.active.store(true, Ordering::Relaxed);
        assert!(first.end().is_empty());
    }
}
