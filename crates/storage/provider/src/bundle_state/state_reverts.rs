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
