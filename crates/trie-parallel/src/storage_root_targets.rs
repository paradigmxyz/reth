use derive_more::{Deref, DerefMut};
use reth_primitives::B256;
use reth_trie::prefix_set::PrefixSet;
use std::collections::HashMap;

/// Target accounts with corresponding prefix sets for storage root calculation.
#[derive(Deref, DerefMut, Debug)]
pub struct StorageRootTargets(HashMap<B256, PrefixSet>);

impl StorageRootTargets {
    /// Create new storage root targets from updated post state accounts
    /// and storage prefix sets.
    ///
    /// NOTE: Since updated accounts and prefix sets always overlap,
    /// it's important that iterator over storage prefix sets takes precedence.
    pub fn new(
        changed_accounts: impl IntoIterator<Item = B256>,
        storage_prefix_sets: impl IntoIterator<Item = (B256, PrefixSet)>,
    ) -> Self {
        Self(
            changed_accounts
                .into_iter()
                .map(|address| (address, PrefixSet::default()))
                .chain(storage_prefix_sets)
                .collect(),
        )
    }
}

impl IntoIterator for StorageRootTargets {
    type Item = (B256, PrefixSet);
    type IntoIter = std::collections::hash_map::IntoIter<B256, PrefixSet>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(feature = "parallel")]
impl rayon::iter::IntoParallelIterator for StorageRootTargets {
    type Iter = rayon::collections::hash_map::IntoIter<B256, PrefixSet>;
    type Item = (B256, PrefixSet);

    fn into_par_iter(self) -> Self::Iter {
        self.0.into_par_iter()
    }
}
