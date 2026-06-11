use crate::{BranchNodeCompact, Nibbles};
use reth_storage_errors::db::DatabaseError;

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
pub mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

/// Depth-first trie iterator.
pub mod depth_first;

/// Mock trie cursor implementations.
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;

/// Metrics tracking trie cursor implementations.
pub mod metrics;
#[cfg(feature = "metrics")]
pub use metrics::TrieCursorMetrics;
pub use metrics::{InstrumentedTrieCursor, TrieCursorMetricsCache};

pub use self::{depth_first::DepthFirstTrieIterator, in_memory::*, subnode::CursorSubNode};
pub use reth_trie_sparse::{TrieCursor, TrieCursorFactory, TrieStorageCursor};

/// Iterator wrapper for `TrieCursor` types
#[derive(Debug)]
pub struct TrieCursorIter<'a, C> {
    cursor: &'a mut C,
    /// The initial value from seek, if any
    initial: Option<Result<(Nibbles, BranchNodeCompact), DatabaseError>>,
}

impl<'a, C> TrieCursorIter<'a, C> {
    /// Create a new iterator from a mutable reference to a cursor. The Iterator will start from the
    /// empty path.
    pub fn new(cursor: &'a mut C) -> Self
    where
        C: TrieCursor,
    {
        let initial = cursor.seek(Nibbles::default()).transpose();
        Self { cursor, initial }
    }
}

impl<'a, C> From<&'a mut C> for TrieCursorIter<'a, C>
where
    C: TrieCursor,
{
    fn from(cursor: &'a mut C) -> Self {
        Self::new(cursor)
    }
}

impl<'a, C> Iterator for TrieCursorIter<'a, C>
where
    C: TrieCursor,
{
    type Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // If we have an initial value from seek, return it first
        if let Some(initial) = self.initial.take() {
            return Some(initial);
        }

        self.cursor.next().transpose()
    }
}
