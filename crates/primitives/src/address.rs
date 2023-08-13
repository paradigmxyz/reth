//! Types used by the `address` namespace to represent addresses and their appearances.
//!
//! See also: <https://github.com/ethereum/execution-apis/pull/456> for specification.

use std::collections::HashSet;

use revm_primitives::{TransactTo, TxEnv};

use crate::{Address, Block, H160};

/// For an address to be found in 32 by sequence, the sequence MUST NOT be smaller than this.
///
/// This excludes vanity addresses that start with 8 empty bytes.
const SMALLEST: &[u8; 32] =
    &hex_literal::hex!("00000000000000000000000000000000000000ffffffffffffffffffffffffff");

/// For an address to be found in 32 by sequence, the sequence MUST start with 12 empty bytes.
const LARGEST: &[u8; 32] =
    &hex_literal::hex!("000000000000000000000000ffffffffffffffffffffffffffffffffffffffff");

/// For an address to be found in 32 by sequence, the sequence MUST NOT end with 8 empty bytes.
const FOUR_EMPTY_BYTES: &[u8; 4] = &[0u8; 4];

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
    let mut set = HashSet::<Address>::new();
    bytes
        .chunks_exact(32)
        .map(|c| {
            let slice: &[u8; 32] = c.try_into().expect("Size of chunk already checked");
            slice
        })
        .filter_map(bytes32_to_possible_address)
        .for_each(|addr| {
            set.insert(addr);
        });
    match set.is_empty() {
        true => None,
        false => Some(set),
    }
}

/// Detects an address if present in 32 bytes.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/456> for the algorithm specification.
pub fn bytes32_to_possible_address(bytes: &[u8; 32]) -> Option<PossibleAddress> {
    if bytes > LARGEST || bytes < SMALLEST {
        return None
    }
    if bytes.ends_with(FOUR_EMPTY_BYTES) {
        return None
    }
    Some(H160(bytes[12..].try_into().expect("Length of bytes already checked")))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Test cases from: <https://github.com/ethereum/execution-apis/pull/456>
    #[test]
    fn valid_addresses_in_bytes32() {
        let maximum =
            hex_literal::hex!("000000000000000000000000ffffffffffffffffffffffffffffffffffffffff");
        let maximum_expected = H160(hex_literal::hex!("ffffffffffffffffffffffffffffffffffffffff"));
        assert_eq!(bytes32_to_possible_address(&maximum).unwrap(), maximum_expected);

        let minimum =
            hex_literal::hex!("00000000000000000000000000000000000000ffffffffffffffffffffffffff");
        let minimum_expected = H160(hex_literal::hex!("00000000000000ffffffffffffffffffffffffff"));
        assert_eq!(bytes32_to_possible_address(&minimum).unwrap(), minimum_expected);

        let max_trailing =
            hex_literal::hex!("000000000000000000000000fffffffffffffffffffffffffffffffff0000000");
        let maxtrail_expected = H160(hex_literal::hex!("fffffffffffffffffffffffffffffffff0000000"));
        assert_eq!(bytes32_to_possible_address(&max_trailing).unwrap(), maxtrail_expected);

        let deposit =
            hex_literal::hex!("00000000000000000000000000000000219ab540356cbb839cbe05303d7705fa");
        let deposit_expected = H160(hex_literal::hex!("00000000219ab540356cbb839cbe05303d7705fa"));
        assert_eq!(bytes32_to_possible_address(&deposit).unwrap(), deposit_expected);

        let nondeposit =
            hex_literal::hex!("0000000000000000000000000219ab540356cbb839cbe05303d7705fa0000000");
        let nondeposit_expected =
            H160(hex_literal::hex!("0219ab540356cbb839cbe05303d7705fa0000000"));
        assert_eq!(bytes32_to_possible_address(&nondeposit).unwrap(), nondeposit_expected);
    }

    #[test]
    fn invalid_addresses_in_bytes32() {
        let invalid_long_trail =
            hex_literal::hex!("000000000000000000000000ffffffffffffffffffffffffffffffff00000000");
        assert!(bytes32_to_possible_address(&invalid_long_trail).is_none());

        let nondeposit_8_trailing =
            hex_literal::hex!("000000000000000000000000219ab540356cbb839cbe05303d7705fa00000000");
        assert!(bytes32_to_possible_address(&nondeposit_8_trailing).is_none());

        let nondeposit_too_few_leading =
            hex_literal::hex!("00000000219ab540356cbb839cbe05303d7705faffffffffffffffffffffffff");
        assert!(bytes32_to_possible_address(&nondeposit_too_few_leading).is_none());
    }

    #[test]
    fn test_addresses_in_bytes() {
        let bytes = hex_literal::hex!("00000000000000000000000000000000219ab540356cbb839cbe05303d7705fa0000000000000000000000000219ab540356cbb839cbe05303d7705fa0000000");
        let expected_1 = H160(hex_literal::hex!("0219ab540356cbb839cbe05303d7705fa0000000"));
        let expected_2 = H160(hex_literal::hex!("00000000219ab540356cbb839cbe05303d7705fa"));
        assert!(bytes_to_possible_addresses(&bytes).unwrap().contains(&expected_1));
        assert!(bytes_to_possible_addresses(&bytes).unwrap().contains(&expected_2));

        let bytes_with_extra = hex_literal::hex!("00000000000000000000000000000000219ab540356cbb839cbe05303d7705fa0000000000000000000000000219ab540356cbb839cbe05303d7705fa000000000000000");
        assert!(bytes_to_possible_addresses(&bytes_with_extra).unwrap().contains(&expected_1));
        assert!(bytes_to_possible_addresses(&bytes_with_extra).unwrap().contains(&expected_2));

        let address_straddling_bytes32_divide = hex_literal::hex!("000000000000000000000000000000000000000000000000219ab540356cbb839cbe05303d7705fa000000000000000000000000000000000000000000000000");
        assert!(bytes_to_possible_addresses(&address_straddling_bytes32_divide).is_none());
    }
}
