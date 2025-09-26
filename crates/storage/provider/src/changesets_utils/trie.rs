use itertools::{merge_join_by, EitherOrBoth};
use reth_db_api::DatabaseError;
use reth_trie::{trie_cursor::TrieCursor, BranchNodeCompact, Nibbles};
use std::cmp::{Ord, Ordering};

/// Combines a sorted iterator of trie node paths and a storage trie cursor into a new
/// iterator which produces the current values of all given paths in the same order.
#[derive(Debug)]
pub struct StorageTrieCurrentValuesIter<'cursor, P, C> {
    /// Sorted iterator of node paths which we want the values of.
    paths: P,
    /// Storage trie cursor.
    cursor: &'cursor mut C,
    /// Current value at the cursor, allows us to treat the cursor as a peekable iterator.
    cursor_current: Option<(Nibbles, BranchNodeCompact)>,
}

impl<'cursor, P, C> StorageTrieCurrentValuesIter<'cursor, P, C>
where
    P: Iterator<Item = Nibbles>,
    C: TrieCursor,
{
    /// Instantiate a [`StorageTrieCurrentValuesIter`] from a sorted paths iterator and a cursor.
    pub fn new(paths: P, cursor: &'cursor mut C) -> Result<Self, DatabaseError> {
        let mut new_self = Self { paths, cursor, cursor_current: None };
        new_self.seek_cursor(Nibbles::default())?;
        Ok(new_self)
    }

    fn seek_cursor(&mut self, path: Nibbles) -> Result<(), DatabaseError> {
        self.cursor_current = self.cursor.seek(path)?;
        Ok(())
    }
}

impl<'cursor, P, C> Iterator for StorageTrieCurrentValuesIter<'cursor, P, C>
where
    P: Iterator<Item = Nibbles>,
    C: TrieCursor,
{
    type Item = Result<(Nibbles, Option<BranchNodeCompact>), DatabaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(curr_path) = self.paths.next() else {
            // If there are no more paths then there is no further possible output.
            return None
        };

        // If the path is ahead of the cursor then seek the cursor forward to catch up. The cursor
        // will seek either to `curr_path` or beyond it.
        if self.cursor_current.as_ref().is_some_and(|(cursor_path, _)| curr_path > *cursor_path) &&
            let Err(err) = self.seek_cursor(curr_path)
        {
            return Some(Err(err))
        }

        // If there is a path but the cursor is empty then that path has no node.
        if self.cursor_current.is_none() {
            return Some(Ok((curr_path, None)))
        }

        let (cursor_path, cursor_node) =
            self.cursor_current.as_mut().expect("already checked for None");

        // There is both a path and a cursor value, compare their paths.
        match curr_path.cmp(cursor_path) {
            Ordering::Less => {
                // If the path is behind the cursor then there is no value for that
                // path, produce None.
                Some(Ok((curr_path, None)))
            }
            Ordering::Equal => {
                // If the target path and cursor's path match then there is a value for that path,
                // return the value. We don't seek the cursor here, that will be handled on the
                // next call to `next` after checking that `paths` isn't None.
                let cursor_node = core::mem::take(cursor_node);
                Some(Ok((*cursor_path, Some(cursor_node))))
            }
            Ordering::Greater => {
                panic!("cursor was seeked to {curr_path:?}, but produced a node at a lower path {cursor_path:?}")
            }
        }
    }
}

/// Returns an iterator which produces the values to be inserted into
/// [`tables::StoragesTrieChangeSets`] for an account whose storage was wiped during a block. It is
/// expected that this is called prior to inserting the block's trie updates.
///
/// ## Arguments
///
/// - `curr_values_of_changed` is an iterator over the current values of all trie nodes modified by
///   the block, ordered by path.
/// - `all_nodes` is an iterator over all existing trie nodes for the account, ordered by path.
///
/// ## Returns
///
/// An iterator of trie node paths and a `Some(node)` (indicating the node was wiped) or a `None`
/// (indicating the node was modified in the block but didn't previously exist. The iterator's
/// results will be ordered by path.
pub fn storage_trie_wiped_changeset_iter(
    curr_values_of_changed: impl Iterator<
        Item = Result<(Nibbles, Option<BranchNodeCompact>), DatabaseError>,
    >,
    all_nodes: impl Iterator<Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>>,
) -> Result<
    impl Iterator<Item = Result<(Nibbles, Option<BranchNodeCompact>), DatabaseError>>,
    DatabaseError,
> {
    let all_nodes = all_nodes.map(|e| e.map(|(nibbles, node)| (nibbles, Some(node))));

    let merged = merge_join_by(curr_values_of_changed, all_nodes, |a, b| match (a, b) {
        (Err(_), _) => Ordering::Less,
        (_, Err(_)) => Ordering::Greater,
        (Ok(a), Ok(b)) => a.0.cmp(&b.0),
    });

    Ok(merged.map(|either_or| match either_or {
        EitherOrBoth::Left(changed) => {
            // A path of a changed node (given in `paths`) which was not found in the database (or
            // there's an error). The current value of this path must be None, otherwise it would
            // have also been returned by the `all_nodes` iter.
            debug_assert!(
                changed.as_ref().is_err() || changed.as_ref().is_ok_and(|(_, node)| node.is_none()),
                "changed node is Some but wasn't returned by `all_nodes` iterator: {changed:?}",
            );
            changed
        }
        EitherOrBoth::Right(wiped) => {
            // A node was found in the db (indicating it was wiped) but was not given in `paths`.
            // Return it as-is.
            wiped
        }
        EitherOrBoth::Both(changed, _wiped) => {
            // A path of a changed node (given in `paths`) was found with a previous value in the
            // database. The changed node must have a value which is equal to the one found by the
            // `all_nodes` iterator. If the changed node had no previous value (None) it wouldn't
            // be returned by `all_nodes` and so would be in the Left branch.
            //
            // Due to the ordering closure passed to `merge_join_by` it's not possible for either
            // value to be an error here.
            debug_assert!(changed.is_ok(), "unreachable error condition: {changed:?}");
            debug_assert_eq!(changed, _wiped);
            changed
        }
    }))
}
