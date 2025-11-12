//! Proof target types for specifying what to prove.

use alloy_primitives::B256;

/// A collection of proof targets specifying accounts and storage slots to prove.
///
/// Targets should be sorted lexicographically for optimal performance.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProofTargets {
    /// Vector of account hashes paired with their storage slot targets.
    /// - Empty vec means prove account only (no storage)
    /// - Non-empty vec means prove account and the specified storage slots
    ///
    /// Storage slots within each account's vec must be sorted lexicographically.
    targets: Vec<(B256, Vec<B256>)>,
}

impl ProofTargets {
    /// Create a new set of proof targets.
    ///
    /// # Panics (debug builds)
    /// Panics in debug builds if the storage slot vecs are not sorted lexicographically.
    pub fn new(targets: Vec<(B256, Vec<B256>)>) -> Self {
        #[cfg(debug_assertions)]
        {
            assert!(
                targets.is_sorted_by_key(|(account, _)| account),
                "Targets must be sorted by lexicographically account",
            );
            for (_, slots) in &targets {
                assert!(slots.is_sorted(), "Storage slots must be sorted lexicographically");
            }
        }
        Self { targets }
    }
}

impl<I> FromIterator<(B256, I)> for ProofTargets
where
    I: IntoIterator<Item = B256>,
{
    fn from_iter<T: IntoIterator<Item = (B256, I)>>(iter: T) -> Self {
        let mut targets: Vec<(B256, Vec<B256>)> = iter
            .into_iter()
            .map(|(account, slots)| {
                let mut slot_vec: Vec<B256> = slots.into_iter().collect();
                slot_vec.sort_unstable();
                (account, slot_vec)
            })
            .collect();
        targets.sort_unstable_by_key(|(account, _)| *account);
        Self::new(targets)
    }
}
