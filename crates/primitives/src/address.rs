//! Types used by the `address` namespace to represent addresses and their appearances.
//!
//! See also: <https://github.com/ethereum/execution-apis/pull/456> for specification.

use std::collections::HashSet;

use revm_primitives::{TransactTo, TxEnv};

use crate::{Address, Block, H160};

/// Detects addresses present in a transaction object
pub fn get_addresses_from_tx(tx: &TxEnv) -> HashSet<Address> {
    let mut set = HashSet::<Address>::new();
    set.insert(tx.caller);
    for (address, _) in &tx.access_list {
        set.insert(*address);
    }
    if let Some(addresses) = bytes_to_possible_addresses(&tx.data) {
        for address in addresses {
            set.insert(address);
        }
    }
    if let TransactTo::Call(address) = tx.transact_to {
        set.insert(H160(*address));
    }
    set
}

/// Addresses present in a block object
#[derive(Debug, Clone)]
pub struct BlockAddresses {
    /// Address in the block "miner" field (miner / producer / beneficiary)
    pub miner: Address,
    /// Addresses in the block "withdrawals" field
    pub withdrawals: Option<HashSet<Address>>,
    /// Unique addresses in the "miner" field of each header in the block "uncles" field.
    pub uncles: Option<HashSet<Address>>,
    /// Unique addresses in the "addess" field of each object in the block "withdrawals" field.
    pub alloc: Option<HashSet<Address>>,
}

/// Detects addresses present in a block object
pub fn get_addresses_from_block(block: &Block) -> BlockAddresses {
    let withdrawals = match &block.withdrawals {
        Some(withdrawals) => {
            let mut set = HashSet::<Address>::new();
            for withdrawal in withdrawals {
                set.insert(withdrawal.address);
            }
            Some(set)
        }
        None => None,
    };
    let uncles = match block.ommers.is_empty() {
        true => None,
        false => {
            let mut set = HashSet::<Address>::new();
            for uncle in &block.ommers {
                set.insert(uncle.beneficiary);
            }
            Some(set)
        }
    };
    BlockAddresses { miner: block.beneficiary, withdrawals, uncles, alloc: None }
}

/// A possible address is used when finding addresses in bytes. It may include false positives.
///
/// Specification: <https://github.com/ethereum/execution-apis/pull/456>
pub type PossibleAddress = Address;

/// A unique set of possible addresses.
pub type PossibleAddresses = HashSet<PossibleAddress>;

/// Searches for and returns addresses that are present in bytes.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/456> for the algorithm specification.
pub fn bytes_to_possible_addresses(bytes: &[u8]) -> Option<PossibleAddresses> {
    if bytes.len() < 20 {
        return None
    }
    // todo
    None
}

/// Detects an address if present in 32 bytes.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/456> for the algorithm specification.
pub fn bytes32_to_possible_address(_logs: &[u8; 32]) -> Option<PossibleAddress> {
    // todo
    None
}
