//! Merkle trie proofs.

use crate::{
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    ChunkedMultiProofTargets, Nibbles,
};
use alloy_primitives::{
    map::{B256Map, B256Set, Entry},
    B256,
};
use core::cmp::Ordering;

/// Represents a single proof target. This is a helper struct to be used when flattening a
/// [`MultiProofTargets`], similar to an `Option<B256>` but preserving information about whether or
/// not the account proof should be fetched.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SingleProofTarget {
    /// Account proof target, no storage proofs.
    Account,
    /// Storage proof target.
    StorageOnly(B256),
    /// Account with storage proof target.
    AccountWithStorage(B256),
}

impl SingleProofTarget {
    /// Returns an [`Option`] containing the storage slot if a storage slot exists for the target.
    pub fn storage_slot(&self) -> Option<B256> {
        match self {
            Self::StorageOnly(slot) | Self::AccountWithStorage(slot) => Some(*slot),
            Self::Account => None,
        }
    }
}

impl PartialOrd for SingleProofTarget {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SingleProofTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_slot = self.storage_slot();
        let other_slot = other.storage_slot();
        self_slot.cmp(&other_slot)
    }
}

/// Represents proof targets that should be fetched.
///
/// This is meant to be constructed by comparing two [`MultiProofTargets`] instances, where one is
/// the already-fetched proof targets, and the other is the new set of proof targets.
///
/// For example:
/// ```
/// use alloy_primitives::{map::B256Map, B256};
/// use reth_trie_common::{FetchedProofTargets, MultiProofTargets};
///
/// let fetched = FetchedProofTargets::accounts([
///     B256::from([1; 32]),
///     B256::from([2; 32]),
///     B256::from([3; 32]),
/// ]);
/// let new = FetchedProofTargets::accounts([
///     B256::from([1; 32]),
///     B256::from([2; 32]),
///     B256::from([4; 32]),
/// ]);
///
/// let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
/// let expected =
///     MultiProofTargets::from_account_targets([(B256::from([4; 32]), Default::default())]);
///
/// assert_eq!(targets_to_fetch, expected);
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MultiProofTargets {
    /// Account and storage multiproof targets
    account_targets: B256Map<B256Set>,

    /// Storage-only targets
    storage_only_targets: B256Map<B256Set>,
}

impl FromIterator<(B256, B256Set)> for MultiProofTargets {
    fn from_iter<T: IntoIterator<Item = (B256, B256Set)>>(iter: T) -> Self {
        Self { account_targets: B256Map::from_iter(iter), storage_only_targets: B256Map::default() }
    }
}

impl MultiProofTargets {
    /// Creates a new [`MultiProofTargets`] instance from iterators over account and storage-only
    /// multiproof targets.
    pub fn new(
        account_targets: impl IntoIterator<Item = (B256, B256Set)>,
        storage_only_targets: impl IntoIterator<Item = (B256, B256Set)>,
    ) -> Self {
        Self {
            account_targets: B256Map::from_iter(account_targets),
            storage_only_targets: B256Map::from_iter(storage_only_targets),
        }
    }

    /// Creates a new [`MultiProofTargets`] instance from an account target map and storage-only
    /// target map directly.
    pub fn from_target_maps(
        account_targets: B256Map<B256Set>,
        storage_only_targets: B256Map<B256Set>,
    ) -> Self {
        Self { account_targets, storage_only_targets }
    }

    /// Creates a new [`MultiProofTargets`] instance from an iterator over account multiproof
    /// targets.
    ///
    /// This leaves the storage-only targets empty.
    pub fn from_account_targets(
        account_targets: impl IntoIterator<Item = (B256, B256Set)>,
    ) -> Self {
        Self {
            account_targets: B256Map::from_iter(account_targets),
            storage_only_targets: B256Map::default(),
        }
    }

    /// Creates a new [`MultiProofTargets`] instance from an iterator over storage-only multiproof
    /// targets.
    ///
    /// This leaves the account targets empty.
    pub fn from_storage_only_targets(
        storage_only_targets: impl IntoIterator<Item = (B256, B256Set)>,
    ) -> Self {
        Self {
            account_targets: B256Map::default(),
            storage_only_targets: B256Map::from_iter(storage_only_targets),
        }
    }

    /// Create `MultiProofTargets` with a single account as a target.
    pub fn account(hashed_address: B256) -> Self {
        Self::accounts([hashed_address])
    }

    /// Create a `MultiProofTargets` with multiple accounts as targets.
    pub fn accounts(hashed_addresses: impl IntoIterator<Item = B256>) -> Self {
        Self::from_account_targets(
            hashed_addresses.into_iter().map(|addr| (addr, B256Set::default())),
        )
    }

    /// Creates a [`PrefixSetMut`] from this [`MultiProofTargets`], for the accounts that have
    /// changed.
    pub fn account_prefix_set(&self) -> PrefixSetMut {
        PrefixSetMut::from(self.account_targets.keys().copied().map(Nibbles::unpack))
    }

    /// Creates a map from hashed address to [`PrefixSetMut`] for storage prefix sets in accounts
    /// that have changed.
    pub fn storage_prefix_set_map(&self) -> B256Map<PrefixSetMut> {
        self.storage_only_targets
            .iter()
            .filter(|&(_hashed_address, slots)| !slots.is_empty())
            .map(|(hashed_address, slots)| {
                (*hashed_address, PrefixSetMut::from(slots.iter().map(Nibbles::unpack)))
            })
            .collect()
    }

    /// Creates a [`TriePrefixSetsMut`] from this [`MultiProofTargets`].
    pub fn trie_prefix_sets(&self) -> TriePrefixSetsMut {
        TriePrefixSetsMut {
            account_prefix_set: self.account_prefix_set(),
            storage_prefix_sets: self.storage_prefix_set_map(),
            destroyed_accounts: Default::default(),
        }
    }

    /// Returns true if the hashed account address is present in the account multiproof set.
    ///
    /// This does not check whether the hashed address is present in the storage-only target set.
    pub fn contains_account_target(&self, hashed_address: &B256) -> bool {
        self.account_targets.contains_key(hashed_address)
    }

    /// Returns the storage proof targets entry for the given hashed address.
    pub fn storage_targets_entry(&mut self, hashed_address: B256) -> Entry<'_, B256, B256Set> {
        self.storage_only_targets.entry(hashed_address)
    }

    /// Returns the account multiproof targets entry for the given hashed address.
    pub fn account_targets_entry(&mut self, hashed_address: B256) -> Entry<'_, B256, B256Set> {
        self.account_targets.entry(hashed_address)
    }

    /// Creates a new `MultiProofTargets` instance from the difference between the already-fetched
    /// [`MultiProofTargets`] instance, and new [`TargetsToFetch`].
    pub fn from_targets_difference(fetched: &B256Map<B256Set>, new: Self) -> Self {
        let mut account_targets = B256Map::default();
        let mut storage_only_targets = B256Map::default();

        // When we have new targets, if we have fetched the account for any of them already, we move
        // it to storage only.
        //
        // Beyond that, we will filter out any storage-only targets that we already have
        for (hashed_address, mut new_slots) in new.account_targets {
            // check if we have already fetched the account
            match fetched.get(&hashed_address) {
                Some(fetched_slots) => {
                    // let's filter out all the slots that we already have
                    new_slots.retain(|slot| !fetched_slots.contains(slot));

                    // if we have the account already and the fetched storage slots are a superset
                    // of the new storage slots, we don't need to fetch anything.
                    //
                    // if the new slots are empty, we also don't need to fetch anything
                    if !new_slots.is_empty() {
                        // we do need to fetch some new slots, but not the account proof
                        storage_only_targets.insert(hashed_address, new_slots);
                    }
                }
                None => {
                    // if we don't have the account then we need to fetch both the account and
                    // storage slots
                    account_targets.insert(hashed_address, new_slots);
                }
            };
        }

        // Now we filter out any storage-only targets that we already have
        storage_only_targets.extend(new.storage_only_targets);
        for (hashed_address, new_slots) in &mut storage_only_targets {
            if let Some(fetched_slots) = fetched.get(hashed_address) {
                // let's filter out all the slots that we already have
                new_slots.retain(|slot| !fetched_slots.contains(slot));
            }
        }

        // Finally, we remove any storage-only targets that are now empty
        storage_only_targets.retain(|_, slots| !slots.is_empty());

        Self { account_targets, storage_only_targets }
    }

    /// Creates a new `MultiProofTargets` instance from the difference between the already-fetched
    /// [`MultiProofTargets`] instance, and new targets from the iterator.
    // TODO: remove this
    pub fn from_difference(
        fetched: &B256Map<B256Set>,
        new: impl IntoIterator<Item = (B256, B256Set)>,
    ) -> Self {
        let mut account_targets = B256Map::default();
        let mut storage_only_targets = B256Map::default();
        for (hashed_address, mut new_slots) in new {
            // check if we have already fetched the account
            match fetched.get(&hashed_address) {
                Some(fetched_slots) => {
                    // let's filter out all the slots that we already have
                    new_slots.retain(|slot| !fetched_slots.contains(slot));

                    // if we have the account already and the fetched storage slots are a superset
                    // of the new storage slots, we don't need to fetch anything.
                    //
                    // if the new slots are empty, we also don't need to fetch anything
                    if !new_slots.is_empty() {
                        // we do need to fetch some new slots, but not the account proof
                        storage_only_targets.insert(hashed_address, new_slots);
                    }
                }
                None => {
                    // if we don't have the account then we need to fetch both the account and
                    // storage slots
                    account_targets.insert(hashed_address, new_slots);
                }
            };
        }

        Self { account_targets, storage_only_targets }
    }

    /// This turns the [`MultiProofTargets`] into a single map, erasing any information about
    /// storage-only proof targets.
    ///
    /// NOTE: This is only provided for compatibility with methods that do not distinguish between
    /// storage-only multiproofs and regular multiproofs.
    pub fn to_targets_map(self) -> B256Map<B256Set> {
        let (mut multiproof_targets, storage_only_targets) = self.into_inner();
        multiproof_targets.extend(storage_only_targets);
        multiproof_targets
    }

    /// Inserts the hashed address and target slots into the account multiproof targets.
    pub fn insert_account_targets(&mut self, hashed_address: B256, slots: B256Set) {
        self.account_targets.entry(hashed_address).or_default().extend(slots);
    }

    /// Inserts a single account target into the account multiproof targets.
    pub fn insert_account_target(&mut self, hashed_address: B256, slot: B256) {
        self.account_targets.entry(hashed_address).or_default().insert(slot);
    }

    /// Inserts a single target into the storage-only multiproof targets.
    pub fn insert_storage_only_target(&mut self, hashed_address: B256, slot: B256) {
        self.storage_only_targets.entry(hashed_address).or_default().insert(slot);
    }

    /// Returns the total number of slots to fetch.
    pub fn total_slots(&self) -> usize {
        self.account_targets.values().map(|slots| slots.len()).sum::<usize>() +
            self.storage_only_targets.values().map(|slots| slots.len()).sum::<usize>()
    }

    /// Returns the number of accounts in _both_ the account and storage-only multiproof target
    /// maps.
    pub fn total_accounts(&self) -> usize {
        self.account_targets.len() + self.storage_only_targets.len()
    }

    /// Returns whether or not the targets to fetch are empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.account_targets.is_empty() && self.storage_only_targets.is_empty()
    }

    /// Splits the [`MultiProofTargets`] into the account and storage-only multiproof targets.
    pub fn into_inner(self) -> (B256Map<B256Set>, B256Map<B256Set>) {
        (self.account_targets, self.storage_only_targets)
    }

    /// Returns a reference to the account multiproof targets.
    #[inline]
    pub fn account_targets(&self) -> &B256Map<B256Set> {
        &self.account_targets
    }

    /// Returns a reference to the storage-only multiproof targets.
    #[inline]
    pub fn storage_only_targets(&self) -> &B256Map<B256Set> {
        &self.storage_only_targets
    }

    /// Returns a reference to the storage targets for the given hashed address, if it exists.
    pub fn get_storage_targets(&self, hashed_address: &B256) -> Option<&B256Set> {
        self.storage_only_targets.get(hashed_address)
    }

    /// Returns true if the account is present in either the account or storage-only
    /// multiproof targets.
    pub fn contains_account(&self, hashed_address: &B256) -> bool {
        self.account_targets.contains_key(hashed_address) ||
            self.storage_only_targets.contains_key(hashed_address)
    }

    /// Returns an iterator that yields chunks of the specified size.
    ///
    /// See [`ChunkedMultiProofTargets`] for more information.
    pub fn chunks(self, size: usize) -> ChunkedMultiProofTargets {
        ChunkedMultiProofTargets::new(self, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_to_fetch_difference_accounts_only() {
        let fetched = FetchedProofTargets::accounts([
            B256::from([1; 32]),
            B256::from([2; 32]),
            B256::from([3; 32]),
        ]);
        let new = FetchedProofTargets::accounts([
            B256::from([1; 32]),
            B256::from([2; 32]),
            B256::from([4; 32]),
        ]);

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected =
            MultiProofTargets::from_account_targets([(B256::from([4; 32]), Default::default())]);

        assert_eq!(targets_to_fetch, expected);
    }

    #[test]
    fn targets_to_fetch_difference_subset_storage() {
        // Here we have slots 1, 2 already fetched, and get proof targets for 1, 2, 3.
        // So we want to fetch only the proof for storage slot 3.
        let fetched = FetchedProofTargets::account_with_slots(
            B256::from([1; 32]),
            [B256::from([1; 32]), B256::from([2; 32])],
        );

        let new = FetchedProofTargets::account_with_slots(
            B256::from([1; 32]),
            [B256::from([1; 32]), B256::from([2; 32]), B256::from([3; 32])],
        );

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected = MultiProofTargets::from_storage_only_targets([(
            B256::from([1; 32]),
            B256Set::from_iter([B256::from([3; 32])]),
        )]);

        assert_eq!(targets_to_fetch, expected);
    }

    #[test]
    fn targets_to_fetch_accounts_to_storage_only() {
        // We have slots 1 and 2 already, but get a proof target for slot 3. In this
        // case we want to fetch the storage proof for slot 3
        let fetched = FetchedProofTargets::accounts([B256::from([1; 32]), B256::from([2; 32])]);
        let new =
            FetchedProofTargets::account_with_slots(B256::from([1; 32]), [B256::from([3; 32])]);

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected = MultiProofTargets::from_storage_only_targets([(
            B256::from([1; 32]),
            B256Set::from_iter([B256::from([3; 32])]),
        )]);

        assert_eq!(targets_to_fetch, expected);
    }

    #[test]
    fn targets_to_fetch_account_with_storage() {
        // here the fetched targets does not have the account we are looking for, and we want to
        // fetch the account and storage proofs for the new account
        let fetched = FetchedProofTargets::accounts([B256::from([1; 32]), B256::from([2; 32])]);
        let new = FetchedProofTargets::account_with_slots(
            B256::from([3; 32]),
            [B256::from([1; 32]), B256::from([2; 32])],
        );

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected = MultiProofTargets::from_account_targets([(
            B256::from([3; 32]),
            B256Set::from_iter([B256::from([1; 32]), B256::from([2; 32])]),
        )]);

        assert_eq!(targets_to_fetch, expected);
    }

    #[test]
    fn targets_to_fetch_account_only() {
        // here the fetched targets does not have the account we are looking for, but the incoming
        // proof targets do not have any storage slots to fetch, so we want to return the account
        // proof only.
        let fetched = FetchedProofTargets::accounts([B256::from([1; 32]), B256::from([2; 32])]);
        let new = FetchedProofTargets::account_with_slots(B256::from([3; 32]), []);

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected =
            MultiProofTargets::from_account_targets([(B256::from([3; 32]), Default::default())]);

        assert_eq!(targets_to_fetch, expected);
    }

    #[test]
    fn targets_to_fetch_empty() {
        // here the fetched targets already have all the proof targets
        // so we don't need to fetch anything
        let fetched = FetchedProofTargets::accounts([B256::from([1; 32]), B256::from([2; 32])]);
        let new = FetchedProofTargets::accounts([B256::from([1; 32])]);

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected = MultiProofTargets::default();

        assert_eq!(targets_to_fetch, expected);
    }

    #[test]
    fn targets_to_fetch_empty_with_storage() {
        // here the fetched targets already have all the proof targets, including the storage slots
        // so we don't need to fetch anything
        let fetched = FetchedProofTargets::account_with_slots(
            B256::from([1; 32]),
            [B256::from([1; 32]), B256::from([2; 32]), B256::from([3; 32])],
        );

        let new = FetchedProofTargets::account_with_slots(
            B256::from([1; 32]),
            [B256::from([1; 32]), B256::from([2; 32])],
        );

        let targets_to_fetch = MultiProofTargets::from_difference(&fetched, new);
        let expected = MultiProofTargets::default();

        assert_eq!(targets_to_fetch, expected);
    }
}
