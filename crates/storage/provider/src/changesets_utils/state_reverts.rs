use alloy_primitives::{Address, B256, U256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
};
use reth_storage_errors::provider::ProviderResult;
use revm_database::states::RevertToSlot;
use std::iter::Peekable;

/// Iterator over storage reverts.
/// See [`StorageRevertsIter::next`] for more details.
#[expect(missing_debug_implementations)]
pub struct StorageRevertsIter<R: Iterator, W: Iterator> {
    reverts: Peekable<R>,
    wiped: Peekable<W>,
}

impl<R, W> StorageRevertsIter<R, W>
where
    R: Iterator<Item = (B256, RevertToSlot)>,
    W: Iterator<Item = (B256, U256)>,
{
    /// Create a new iterator over storage reverts.
    pub fn new(
        reverts: impl IntoIterator<IntoIter = R>,
        wiped: impl IntoIterator<IntoIter = W>,
    ) -> Self {
        Self { reverts: reverts.into_iter().peekable(), wiped: wiped.into_iter().peekable() }
    }

    /// Consume next revert and return it.
    fn next_revert(&mut self) -> Option<(B256, RevertToSlot)> {
        self.reverts.next()
    }

    /// Consume next wiped storage and return it.
    fn next_wiped(&mut self) -> Option<(B256, U256)> {
        self.wiped.next()
    }
}

impl<R, W> Iterator for StorageRevertsIter<R, W>
where
    R: Iterator<Item = (B256, RevertToSlot)>,
    W: Iterator<Item = (B256, U256)>,
{
    type Item = (B256, RevertToSlot);

    /// Iterate over storage reverts and wiped entries and return items in the sorted order.
    /// NOTE: The implementation assumes that inner iterators are already sorted.
    fn next(&mut self) -> Option<Self::Item> {
        match (self.reverts.peek(), self.wiped.peek()) {
            (Some(revert), Some(wiped)) => {
                // Compare the keys and return the lesser.
                use std::cmp::Ordering;
                match revert.0.cmp(&wiped.0) {
                    Ordering::Less => self.next_revert(),
                    Ordering::Greater => {
                        // For wiped entries that don't have explicit reverts,
                        // we should preserve the original value from storage
                        let (key, value) = *wiped;
                        self.next_wiped();
                        Some((key, RevertToSlot::Some(value)))
                    }
                    Ordering::Equal => {
                        // Keys are the same, prefer the revert value
                        let (key, revert_to) = *revert;

                        // Consume both values from inner iterators
                        self.next_revert();
                        self.next_wiped();

                        Some((key, revert_to))
                    }
                }
            }
            (Some(_revert), None) => self.next_revert(),
            (None, Some(_wiped)) => {
                // For wiped entries without reverts, preserve the original value
                self.next_wiped().map(|(key, value)| (key, RevertToSlot::Some(value)))
            }
            (None, None) => None,
        }
    }
}

/// Iterator over wiped storage entries directly from a database cursor.
/// This avoids collecting entries into an intermediate Vec.
///
/// # Performance
/// This iterator processes storage entries on-demand directly from the cursor,
/// eliminating the need for intermediate Vec allocation and reducing memory pressure.
#[derive(Debug)]
pub struct StorageWipedEntriesIter<'cursor, C> {
    cursor: &'cursor mut C,
    first_entry: Option<(B256, U256)>,
    exhausted: bool,
}

impl<'cursor, C> StorageWipedEntriesIter<'cursor, C>
where
    C: DbDupCursorRO<tables::PlainStorageState> + DbCursorRO<tables::PlainStorageState>,
{
    /// Create a new iterator over wiped storage entries for the given address.
    ///
    /// This will seek to the first entry for the address and prepare the iterator.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cursor operation fails.
    pub fn new(cursor: &'cursor mut C, address: Address) -> ProviderResult<Self> {
        // Seek to the first entry for this address
        let first_entry = cursor.seek_exact(address)?.map(|(_, entry)| (entry.key, entry.value));

        Ok(Self { cursor, first_entry, exhausted: first_entry.is_none() })
    }
}

impl<'cursor, C> Iterator for StorageWipedEntriesIter<'cursor, C>
where
    C: DbDupCursorRO<tables::PlainStorageState> + DbCursorRO<tables::PlainStorageState>,
{
    type Item = (B256, U256);

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }

        // If we have a first entry saved, return it and clear it
        if let Some(entry) = self.first_entry.take() {
            return Some(entry);
        }

        // Get the next duplicate value for the current address
        match self.cursor.next_dup_val() {
            Ok(Some(entry)) => Some((entry.key, entry.value)),
            Ok(None) => {
                // No more entries for this address
                self.exhausted = true;
                None
            }
            Err(_) => {
                // On error, mark as exhausted and return None
                // The error will be handled at a higher level
                self.exhausted = true;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_reverts_iter_empty() {
        // Create empty sample data for reverts and wiped entries.
        let reverts: Vec<(B256, RevertToSlot)> = vec![];
        let wiped: Vec<(B256, U256)> = vec![];

        // Create the iterator with the empty data.
        let iter = StorageRevertsIter::new(reverts, wiped);

        // Iterate and collect results into a vector for verification.
        let results: Vec<_> = iter.collect();

        // Verify that the results are empty.
        assert_eq!(results, vec![]);
    }

    #[test]
    fn test_storage_reverts_iter_reverts_only() {
        // Create sample data for only reverts.
        let reverts = vec![
            (B256::from_slice(&[4; 32]), RevertToSlot::Destroyed),
            (B256::from_slice(&[5; 32]), RevertToSlot::Some(U256::from(40))),
        ];

        // Create the iterator with only reverts and no wiped entries.
        let iter = StorageRevertsIter::new(reverts, vec![]);

        // Iterate and collect results into a vector for verification.
        let results: Vec<_> = iter.collect();

        // Verify the output order and values.
        assert_eq!(
            results,
            vec![
                (B256::from_slice(&[4; 32]), RevertToSlot::Destroyed),
                (B256::from_slice(&[5; 32]), RevertToSlot::Some(U256::from(40))),
            ]
        );
    }

    #[test]
    fn test_storage_reverts_iter_wiped_only() {
        // Create sample data for only wiped entries.
        let wiped = vec![
            (B256::from_slice(&[6; 32]), U256::from(50)),
            (B256::from_slice(&[7; 32]), U256::from(60)),
        ];

        // Create the iterator with only wiped entries and no reverts.
        let iter = StorageRevertsIter::new(vec![], wiped);

        // Iterate and collect results into a vector for verification.
        let results: Vec<_> = iter.collect();

        // Verify the output order and values - should preserve original values
        assert_eq!(
            results,
            vec![
                (B256::from_slice(&[6; 32]), RevertToSlot::Some(U256::from(50))),
                (B256::from_slice(&[7; 32]), RevertToSlot::Some(U256::from(60))),
            ]
        );
    }

    #[test]
    fn test_storage_reverts_iter_interleaved() {
        // Create sample data for interleaved reverts and wiped entries.
        let reverts = vec![
            (B256::from_slice(&[8; 32]), RevertToSlot::Some(U256::from(70))),
            (B256::from_slice(&[9; 32]), RevertToSlot::Some(U256::from(80))),
            // Some higher key than wiped
            (B256::from_slice(&[15; 32]), RevertToSlot::Some(U256::from(90))),
        ];

        let wiped = vec![
            (B256::from_slice(&[8; 32]), U256::from(75)), // Same key as revert
            (B256::from_slice(&[10; 32]), U256::from(85)), // Wiped with new key
        ];

        // Create the iterator with the sample data.
        let iter = StorageRevertsIter::new(reverts, wiped);

        // Iterate and collect results into a vector for verification.
        let results: Vec<_> = iter.collect();

        // Verify the output order and values.
        assert_eq!(
            results,
            vec![
                (B256::from_slice(&[8; 32]), RevertToSlot::Some(U256::from(70))), /* Revert takes priority. */
                (B256::from_slice(&[9; 32]), RevertToSlot::Some(U256::from(80))), /* Only revert
                                                                                   * present. */
                (B256::from_slice(&[10; 32]), RevertToSlot::Some(U256::from(85))), /* Wiped entry preserves value */
                (B256::from_slice(&[15; 32]), RevertToSlot::Some(U256::from(90))), /* Greater revert entry */
            ]
        );
    }
}
