use crate::{Address, H256};

/// A list of addresses and storage keys that the transaction plans to access.
/// Accesses outside the list are possible, but become more expensive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessListItem {
    /// Account addresses that would be loaded at the start of execution
    pub address: Address,
    /// Keys of storage that would be loaded at the start of execution
    pub storage_keys: Vec<H256>,
}

/// AccessList as defined in EIP-2930
pub type AccessList = Vec<AccessListItem>;
