use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};

/// Limits a pruner run by either the number of entries (rows in the database) that can be deleted
/// or the time it can run.
#[derive(Debug, Clone, Default)]
pub struct PruneLimiter {
    /// Maximum entries (rows in the database) to delete from the database per block.
    deleted_entries_limit: Option<PruneDeletedEntriesLimit>,
    /// Maximum duration of one prune run.
    time_limit: Option<PruneTimeLimit>,
}

#[derive(Debug, Clone)]
struct PruneDeletedEntriesLimit {
    /// Maximum entries (rows in the database) to delete from the database.
    limit: usize,
    /// Current number of entries (rows in the database) that have been deleted.
    deleted: usize,
}

impl PruneDeletedEntriesLimit {
    const fn new(limit: usize) -> Self {
        Self { limit, deleted: 0 }
    }

    const fn is_limit_reached(&self) -> bool {
        self.deleted >= self.limit
    }
}

#[derive(Debug, Clone)]
struct PruneTimeLimit {
    /// Maximum duration of one prune run.
    limit: Duration,
    /// Time when the prune run has started.
    start: Instant,
}

impl PruneTimeLimit {
    fn new(limit: Duration) -> Self {
        Self { limit, start: Instant::now() }
    }

    fn is_limit_reached(&self) -> bool {
        self.start.elapsed() > self.limit
    }
}

impl PruneLimiter {
    /// Sets the limit on the number of deleted entries (rows in the database).
    /// If the limit was already set, it will be overwritten.
    pub fn set_deleted_entries_limit(mut self, limit: usize) -> Self {
        if let Some(deleted_entries_limit) = self.deleted_entries_limit.as_mut() {
            deleted_entries_limit.limit = limit;
        } else {
            self.deleted_entries_limit = Some(PruneDeletedEntriesLimit::new(limit));
        }

        self
    }

    /// Sets the limit on the number of deleted entries (rows in the database) to a biggest
    /// multiple of the given denominator that is smaller than the existing limit.
    ///
    /// If the limit wasn't set, does nothing.
    pub fn floor_deleted_entries_limit_to_multiple_of(mut self, denominator: NonZeroUsize) -> Self {
        if let Some(deleted_entries_limit) = self.deleted_entries_limit.as_mut() {
            deleted_entries_limit.limit =
                (deleted_entries_limit.limit / denominator) * denominator.get();
        }

        self
    }

    /// Returns `true` if the limit on the number of deleted entries (rows in the database) is
    /// reached.
    pub fn is_deleted_entries_limit_reached(&self) -> bool {
        self.deleted_entries_limit.as_ref().map_or(false, |limit| limit.is_limit_reached())
    }

    /// Increments the number of deleted entries by the given number.
    pub fn increment_deleted_entries_count_by(&mut self, entries: usize) {
        if let Some(limit) = self.deleted_entries_limit.as_mut() {
            limit.deleted += entries;
        }
    }

    /// Increments the number of deleted entries by one.
    pub fn increment_deleted_entries_count(&mut self) {
        self.increment_deleted_entries_count_by(1)
    }

    /// Returns the number of deleted entries left before the limit is reached.
    pub fn deleted_entries_limit_left(&self) -> Option<usize> {
        self.deleted_entries_limit.as_ref().map(|limit| limit.limit - limit.deleted)
    }

    /// Returns the limit on the number of deleted entries (rows in the database).
    pub fn deleted_entries_limit(&self) -> Option<usize> {
        self.deleted_entries_limit.as_ref().map(|limit| limit.limit)
    }

    /// Sets the time limit.
    pub fn set_time_limit(mut self, limit: Duration) -> Self {
        self.time_limit = Some(PruneTimeLimit::new(limit));

        self
    }

    /// Returns `true` if time limit is reached.
    pub fn is_time_limit_reached(&self) -> bool {
        self.time_limit.as_ref().map_or(false, |limit| limit.is_limit_reached())
    }

    /// Returns `true` if any limit is reached.
    pub fn is_limit_reached(&self) -> bool {
        self.is_deleted_entries_limit_reached() || self.is_time_limit_reached()
    }
}
