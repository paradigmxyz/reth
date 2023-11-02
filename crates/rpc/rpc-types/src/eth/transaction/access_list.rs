use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

/// A list of addresses and storage keys that the transaction plans to access.
/// Accesses outside the list are possible, but become more expensive.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccessListItem {
    /// Account addresses that would be loaded at the start of execution
    pub address: Address,
    /// Keys of storage that would be loaded at the start of execution
    pub storage_keys: Vec<B256>,
}

/// AccessList as defined in EIP-2930
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct AccessList(pub Vec<AccessListItem>);

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
        self.0.iter().map(|item| {
            (
                item.address,
                item.storage_keys.iter().map(|slot| U256::from_be_bytes(slot.0)).collect(),
            )
        })
    }
}

/// Access list with gas used appended.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccessListWithGasUsed {
    /// List with accounts accessed during transaction.
    pub access_list: AccessList,
    /// Estimated gas used with access list.
    pub gas_used: U256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_list_serde() {
        let list = AccessList(vec![
            AccessListItem { address: Address::ZERO, storage_keys: vec![B256::ZERO] },
            AccessListItem { address: Address::ZERO, storage_keys: vec![B256::ZERO] },
        ]);
        let json = serde_json::to_string(&list).unwrap();
        let list2 = serde_json::from_str::<AccessList>(&json).unwrap();
        assert_eq!(list, list2);
    }

    #[test]
    fn access_list_with_gas_used() {
        let list = AccessListWithGasUsed {
            access_list: AccessList(vec![
                AccessListItem { address: Address::ZERO, storage_keys: vec![B256::ZERO] },
                AccessListItem { address: Address::ZERO, storage_keys: vec![B256::ZERO] },
            ]),
            gas_used: U256::from(100),
        };
        let json = serde_json::to_string(&list).unwrap();
        let list2 = serde_json::from_str::<AccessListWithGasUsed>(&json).unwrap();
        assert_eq!(list, list2);
    }
}
