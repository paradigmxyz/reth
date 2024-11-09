use alloy_primitives::{B256, U256};
use revm::db::states::RevertToSlot;
use std::iter::Peekable;

/// Iterator over storage reverts.
/// See [`StorageRevertsIter::next`] for more details.
#[allow(missing_debug_implementations)]
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
    fn next_revert(&mut self) -> Option<(B256, U256)> {
        self.reverts.next().map(|(key, revert)| (key, revert.to_previous_value()))
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
    type Item = (B256, U256);

    /// Iterate over storage reverts and wiped entries and return items in the sorted order.
    /// NOTE: The implementation assumes that inner iterators are already sorted.
    fn next(&mut self) -> Option<Self::Item> {
        match (self.reverts.peek(), self.wiped.peek()) {
            (Some(revert), Some(wiped)) => {
                // Compare the keys and return the lesser.
                use std::cmp::Ordering;
                match revert.0.cmp(&wiped.0) {
                    Ordering::Less => self.next_revert(),
                    Ordering::Greater => self.next_wiped(),
                    Ordering::Equal => {
                        // Keys are the same, decide which one to return.
                        let (key, revert_to) = *revert;

                        let value = match revert_to {
                            // If the slot is some, prefer the revert value.
                            RevertToSlot::Some(value) => value,
                            // If the slot was destroyed, prefer the database value.
                            RevertToSlot::Destroyed => wiped.1,
                        };

                        // Consume both values from inner iterators.
                        self.next_revert();
                        self.next_wiped();

                        Some((key, value))
                    }
                }
            }
            (Some(_revert), None) => self.next_revert(),
            (None, Some(_wiped)) => self.next_wiped(),
            (None, None) => None,
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
                (B256::from_slice(&[4; 32]), U256::ZERO), // Revert slot previous value
                (B256::from_slice(&[5; 32]), U256::from(40)), // Only revert present.
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

        // Verify the output order and values.
        assert_eq!(
            results,
            vec![
                (B256::from_slice(&[6; 32]), U256::from(50)), // Only wiped present.
                (B256::from_slice(&[7; 32]), U256::from(60)), // Only wiped present.
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
                (B256::from_slice(&[8; 32]), U256::from(70)), // Revert takes priority.
                (B256::from_slice(&[9; 32]), U256::from(80)), // Only revert present.
                (B256::from_slice(&[10; 32]), U256::from(85)), // Wiped entry.
                (B256::from_slice(&[15; 32]), U256::from(90)), // WGreater revert entry
            ]
        );
    }
}
