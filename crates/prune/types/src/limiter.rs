use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};

/// Limits a pruner run by either the number of entries (rows in the database) that can be deleted
/// or the time it can run.
#[derive(Debug, Clone, Default)]
pub struct PruneLimiter {
    /// Maximum entries (rows in the database) to delete from the database per run.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_prune_deleted_entries_limit_initial_state() {
        let limit_tracker = PruneDeletedEntriesLimit::new(10);
        // Limit should be set properly
        assert_eq!(limit_tracker.limit, 10);
        // No entries should be deleted
        assert_eq!(limit_tracker.deleted, 0);
        assert!(!limit_tracker.is_limit_reached());
    }

    #[test]
    fn test_prune_deleted_entries_limit_is_limit_reached() {
        // Test when the deleted entries are less than the limit
        let mut limit_tracker = PruneDeletedEntriesLimit::new(5);
        limit_tracker.deleted = 3;
        assert!(!limit_tracker.is_limit_reached());

        // Test when the deleted entries are equal to the limit
        limit_tracker.deleted = 5;
        assert!(limit_tracker.is_limit_reached());

        // Test when the deleted entries exceed the limit
        limit_tracker.deleted = 6;
        assert!(limit_tracker.is_limit_reached());
    }

    #[test]
    fn test_prune_time_limit_initial_state() {
        let time_limit = PruneTimeLimit::new(Duration::from_secs(10));
        // The limit should be set correctly
        assert_eq!(time_limit.limit, Duration::from_secs(10));
        // The elapsed time should be very small right after creation
        assert!(time_limit.start.elapsed() < Duration::from_secs(1));
        // Limit should not be reached initially
        assert!(!time_limit.is_limit_reached());
    }

    #[test]
    fn test_prune_time_limit_is_limit_reached() {
        let time_limit = PruneTimeLimit::new(Duration::from_millis(50));

        // Simulate waiting for some time (less than the limit)
        std::thread::sleep(Duration::from_millis(30));
        assert!(!time_limit.is_limit_reached());

        // Simulate waiting for time greater than the limit
        std::thread::sleep(Duration::from_millis(30));
        assert!(time_limit.is_limit_reached());
    }

    #[test]
    fn test_set_deleted_entries_limit_initial_state() {
        let pruner = PruneLimiter::default().set_deleted_entries_limit(100);
        // The deleted_entries_limit should be set with the correct limit
        assert!(pruner.deleted_entries_limit.is_some());
        let deleted_entries_limit = pruner.deleted_entries_limit.unwrap();
        assert_eq!(deleted_entries_limit.limit, 100);
        // The deleted count should be initially zero
        assert_eq!(deleted_entries_limit.deleted, 0);
        // The limit should not be reached initially
        assert!(!deleted_entries_limit.is_limit_reached());
    }

    #[test]
    fn test_set_deleted_entries_limit_overwrite_existing() {
        let mut pruner = PruneLimiter::default().set_deleted_entries_limit(50);
        // Overwrite the existing limit
        pruner = pruner.set_deleted_entries_limit(200);

        assert!(pruner.deleted_entries_limit.is_some());
        let deleted_entries_limit = pruner.deleted_entries_limit.unwrap();
        // Check that the limit has been overwritten correctly
        assert_eq!(deleted_entries_limit.limit, 200);
        // Deleted count should still be zero
        assert_eq!(deleted_entries_limit.deleted, 0);
        assert!(!deleted_entries_limit.is_limit_reached());
    }

    #[test]
    fn test_set_deleted_entries_limit_when_limit_is_reached() {
        let mut pruner = PruneLimiter::default().set_deleted_entries_limit(5);
        assert!(pruner.deleted_entries_limit.is_some());
        let mut deleted_entries_limit = pruner.deleted_entries_limit.clone().unwrap();

        // Simulate deletion of entries
        deleted_entries_limit.deleted = 5;
        assert!(deleted_entries_limit.is_limit_reached());

        // Overwrite the limit and check if it resets correctly
        pruner = pruner.set_deleted_entries_limit(10);
        deleted_entries_limit = pruner.deleted_entries_limit.unwrap();
        assert_eq!(deleted_entries_limit.limit, 10);
        // Deletion count should reset
        assert_eq!(deleted_entries_limit.deleted, 0);
        assert!(!deleted_entries_limit.is_limit_reached());
    }

    #[test]
    fn test_floor_deleted_entries_limit_to_multiple_of() {
        let limiter = PruneLimiter::default().set_deleted_entries_limit(15);
        let denominator = NonZeroUsize::new(4).unwrap();

        // Floor limit to the largest multiple of 4 less than or equal to 15 (that is 12)
        let updated_limiter = limiter.floor_deleted_entries_limit_to_multiple_of(denominator);
        assert_eq!(updated_limiter.deleted_entries_limit.unwrap().limit, 12);

        // Test when the limit is already a multiple of the denominator
        let limiter = PruneLimiter::default().set_deleted_entries_limit(16);
        let updated_limiter = limiter.floor_deleted_entries_limit_to_multiple_of(denominator);
        assert_eq!(updated_limiter.deleted_entries_limit.unwrap().limit, 16);

        // Test when there's no limit set (should not panic)
        let limiter = PruneLimiter::default();
        let updated_limiter = limiter.floor_deleted_entries_limit_to_multiple_of(denominator);
        assert!(updated_limiter.deleted_entries_limit.is_none());
    }

    #[test]
    fn test_is_deleted_entries_limit_reached() {
        // Limit is not set, should return false
        let limiter = PruneLimiter::default();
        assert!(!limiter.is_deleted_entries_limit_reached());

        // Limit is set but not reached, should return false
        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
        limiter.deleted_entries_limit.as_mut().unwrap().deleted = 5;
        // 5 entries deleted out of 10
        assert!(!limiter.is_deleted_entries_limit_reached());

        // Limit is reached, should return true
        limiter.deleted_entries_limit.as_mut().unwrap().deleted = 10;
        // 10 entries deleted out of 10
        assert!(limiter.is_deleted_entries_limit_reached());

        // Deleted entries exceed the limit, should return true
        limiter.deleted_entries_limit.as_mut().unwrap().deleted = 12;
        // 12 entries deleted out of 10
        assert!(limiter.is_deleted_entries_limit_reached());
    }

    #[test]
    fn test_increment_deleted_entries_count_by() {
        // Increment when no limit is set
        let mut limiter = PruneLimiter::default();
        limiter.increment_deleted_entries_count_by(5);
        assert_eq!(limiter.deleted_entries_limit.as_ref().map(|l| l.deleted), None); // Still None

        // Increment when limit is set
        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
        limiter.increment_deleted_entries_count_by(3);
        assert_eq!(limiter.deleted_entries_limit.as_ref().unwrap().deleted, 3); // Now 3 deleted

        // Increment again
        limiter.increment_deleted_entries_count_by(2);
        assert_eq!(limiter.deleted_entries_limit.as_ref().unwrap().deleted, 5); // Now 5 deleted
    }

    #[test]
    fn test_increment_deleted_entries_count() {
        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(5);
        assert_eq!(limiter.deleted_entries_limit.as_ref().unwrap().deleted, 0); // Initially 0

        limiter.increment_deleted_entries_count(); // Increment by 1
        assert_eq!(limiter.deleted_entries_limit.as_ref().unwrap().deleted, 1); // Now 1
    }

    #[test]
    fn test_deleted_entries_limit_left() {
        // Test when limit is set and some entries are deleted
        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
        limiter.increment_deleted_entries_count_by(3); // Simulate 3 deleted entries
        assert_eq!(limiter.deleted_entries_limit_left(), Some(7)); // 10 - 3 = 7

        // Test when no entries are deleted
        limiter = PruneLimiter::default().set_deleted_entries_limit(5);
        assert_eq!(limiter.deleted_entries_limit_left(), Some(5)); // 5 - 0 = 5

        // Test when limit is reached
        limiter.increment_deleted_entries_count_by(5); // Simulate deleting 5 entries
        assert_eq!(limiter.deleted_entries_limit_left(), Some(0)); // 5 - 5 = 0

        // Test when limit is not set
        limiter = PruneLimiter::default(); // No limit set
        assert_eq!(limiter.deleted_entries_limit_left(), None); // Should be None
    }

    #[test]
    fn test_set_time_limit() {
        // Create a PruneLimiter instance with no time limit set
        let mut limiter = PruneLimiter::default();

        // Set a time limit of 5 seconds
        limiter = limiter.set_time_limit(Duration::new(5, 0));

        // Verify that the time limit is set correctly
        assert!(limiter.time_limit.is_some());
        let time_limit = limiter.time_limit.as_ref().unwrap();
        assert_eq!(time_limit.limit, Duration::new(5, 0));
        // Ensure the start time is recent
        assert!(time_limit.start.elapsed() < Duration::new(1, 0));
    }

    #[test]
    fn test_is_time_limit_reached() {
        // Create a PruneLimiter instance and set a time limit of 10 milliseconds
        let mut limiter = PruneLimiter::default();

        // Time limit should not be reached initially
        assert!(!limiter.is_time_limit_reached(), "Time limit should not be reached yet");

        limiter = limiter.set_time_limit(Duration::new(0, 10_000_000)); // 10 milliseconds

        // Sleep for 5 milliseconds (less than the time limit)
        sleep(Duration::new(0, 5_000_000)); // 5 milliseconds
        assert!(!limiter.is_time_limit_reached(), "Time limit should not be reached yet");

        // Sleep for an additional 10 milliseconds (totaling 15 milliseconds)
        sleep(Duration::new(0, 10_000_000)); // 10 milliseconds
        assert!(limiter.is_time_limit_reached(), "Time limit should be reached now");
    }

    #[test]
    fn test_is_limit_reached() {
        // Create a PruneLimiter instance
        let mut limiter = PruneLimiter::default();

        // Test when no limits are set
        assert!(!limiter.is_limit_reached(), "Limit should not be reached with no limits set");

        // Set a deleted entries limit
        limiter = limiter.set_deleted_entries_limit(5);
        assert!(
            !limiter.is_limit_reached(),
            "Limit should not be reached when deleted entries are less than limit"
        );

        // Increment deleted entries count to reach the limit
        limiter.increment_deleted_entries_count_by(5);
        assert!(
            limiter.is_limit_reached(),
            "Limit should be reached when deleted entries equal the limit"
        );

        // Reset the limiter
        limiter = PruneLimiter::default();

        // Set a time limit and check
        limiter = limiter.set_time_limit(Duration::new(0, 10_000_000)); // 10 milliseconds

        // Sleep for 5 milliseconds (less than the time limit)
        sleep(Duration::new(0, 5_000_000)); // 5 milliseconds
        assert!(
            !limiter.is_limit_reached(),
            "Limit should not be reached when time limit not reached"
        );

        // Sleep for another 10 milliseconds (totaling 15 milliseconds)
        sleep(Duration::new(0, 10_000_000)); // 10 milliseconds
        assert!(limiter.is_limit_reached(), "Limit should be reached when time limit is reached");
    }
}
