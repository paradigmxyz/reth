use crate::{Address, B256};
use alloy_primitives::U256;
use alloy_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};
use reth_codecs::{main_codec, Compact};
use std::{
    mem,
    ops::{Deref, DerefMut},
};

/// Represents a list of addresses and storage keys that a transaction plans to access.
///
/// Accesses outside this list incur higher costs due to gas charging.
///
/// This structure is part of [EIP-2930](https://eips.ethereum.org/EIPS/eip-2930), introducing an optional access list for Ethereum transactions.
///
/// The access list allows pre-specifying and pre-paying for accounts and storage
/// slots, mitigating risks introduced by [EIP-2929](https://eips.ethereum.org/EIPS/e).
#[main_codec(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default, RlpDecodable, RlpEncodable)]
#[serde(rename_all = "camelCase")]
pub struct AccessListItem {
    /// Account address that would be loaded at the start of execution
    pub address: Address,
    /// The storage keys to be loaded at the start of execution.
    ///
    /// Each key is a 32-byte value representing a specific storage slot.
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::collection::vec(proptest::arbitrary::any::<B256>(), 0..=20)"
        )
    )]
    pub storage_keys: Vec<B256>,
}

impl AccessListItem {
    /// Calculates a heuristic for the in-memory size of the [AccessListItem].
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<Address>() + self.storage_keys.capacity() * mem::size_of::<B256>()
    }
}

/// AccessList as defined in [EIP-2930](https://eips.ethereum.org/EIPS/eip-2930)
#[main_codec(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default, RlpDecodableWrapper, RlpEncodableWrapper)]
pub struct AccessList(
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::collection::vec(proptest::arbitrary::any::<AccessListItem>(), 0..=20)"
        )
    )]
    pub Vec<AccessListItem>,
);

impl AccessList {
    /// Converts the list into a vec, expected by revm
    pub fn flattened(&self) -> Vec<(Address, Vec<U256>)> {
        self.flatten().collect()
    }

    /// Consumes the type and converts the list into a vec, expected by revm
    pub fn into_flattened(self) -> Vec<(Address, Vec<U256>)> {
        self.into_flatten().collect()
    }

    /// Consumes the type and returns an iterator over the list's addresses and storage keys.
    pub fn into_flatten(self) -> impl Iterator<Item = (Address, Vec<U256>)> {
        self.0.into_iter().map(|item| {
            (
                item.address,
                item.storage_keys.into_iter().map(|slot| U256::from_be_bytes(slot.0)).collect(),
            )
        })
    }

    /// Returns an iterator over the list's addresses and storage keys.
    pub fn flatten(&self) -> impl Iterator<Item = (Address, Vec<U256>)> + '_ {
        self.iter().map(|item| {
            (
                item.address,
                item.storage_keys.iter().map(|slot| U256::from_be_bytes(slot.0)).collect(),
            )
        })
    }

    /// Calculates a heuristic for the in-memory size of the [AccessList].
    #[inline]
    pub fn size(&self) -> usize {
        // take into account capacity
        self.iter().map(AccessListItem::size).sum::<usize>() +
            self.capacity() * mem::size_of::<AccessListItem>()
    }

    /// Returns the position of the given address in the access list, if present.
    pub fn index_of_address(&self, address: Address) -> Option<usize> {
        self.iter().position(|item| item.address == address)
    }

    /// Checks if a specific storage slot within an account is present in the access list.
    ///
    /// Returns a tuple with flags for the presence of the account and the slot.
    pub fn contains(&self, address: Address, slot: B256) -> (bool, bool) {
        self.index_of_address(address)
            .map_or((false, false), |idx| (true, self.contains_storage_key_at_index(slot, idx)))
    }

    /// Checks if the access list contains the specified address.
    pub fn contains_address(&self, address: Address) -> bool {
        self.iter().any(|item| item.address == address)
    }

    /// Checks if the storage keys at the given index within an account are present in the access
    /// list.
    pub fn contains_storage_key_at_index(&self, slot: B256, index: usize) -> bool {
        self.get(index).map_or(false, |entry| {
            entry.storage_keys.iter().any(|storage_key| *storage_key == slot)
        })
    }

    /// Adds an address to the access list and returns `true` if the operation results in a change,
    /// indicating that the address was not previously present.
    pub fn add_address(&mut self, address: Address) -> bool {
        !self.contains_address(address) && {
            self.push(AccessListItem { address, storage_keys: Vec::new() });
            true
        }
    }
}

impl Deref for AccessList {
    type Target = Vec<AccessListItem>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AccessList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<reth_rpc_types::AccessList> for AccessList {
    #[inline]
    fn from(value: reth_rpc_types::AccessList) -> Self {
        AccessList(
            value
                .0
                .into_iter()
                .map(|item| AccessListItem {
                    address: item.address,
                    storage_keys: item.storage_keys,
                })
                .collect(),
        )
    }
}

impl From<AccessList> for reth_rpc_types::AccessList {
    #[inline]
    fn from(value: AccessList) -> Self {
        reth_rpc_types::AccessList(
            value
                .0
                .into_iter()
                .map(|item| reth_rpc_types::AccessListItem {
                    address: item.address,
                    storage_keys: item.storage_keys,
                })
                .collect(),
        )
    }
}
