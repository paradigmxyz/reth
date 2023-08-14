//! Types used by the `address` namespace to represent addresses and their appearances.
//!
//! See also: <https://github.com/ethereum/execution-apis/pull/456> for specification.

use std::ops::{BitAnd, BitOr};

use crate::TxNumber;

const MASK_0: u64 = 0b0000111111111111111111111111111111111111111111111111111111111111;
const MASK_1: u64 = 0b1000111111111111111111111111111111111111111111111111111111111111;
const MASK_2: u64 = 0b0100111111111111111111111111111111111111111111111111111111111111;
const MASK_3: u64 = 0b0010111111111111111111111111111111111111111111111111111111111111;
const MASK_4: u64 = 0b0001111111111111111111111111111111111111111111111111111111111111;
const MASK_5: u64 = 0b1111000000000000000000000000000000000000000000000000000000000000;

/// Represents a transaction or location within a block using bitflags in upper 4 bits.
///
/// Both intra-transaction and extra-transaction address appearances are encoded using
/// global transaction numbers.
///
/// A block in reth is defined as a transaction number and a
/// count of transactions. By adding bitflags to a transaction one can refer to a block.
///
/// Flags should not be combined. E.g., A withdrawal and a miner reward should be stored separately.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddressLocation(TxNumber);

impl AddressLocation {
    /// Creates a new transaction number that has additional encoded information in upper bits.
    pub fn new(tx: TxNumber, kind: LocationInBlock) -> Self {
        AddressLocation(match kind {
            LocationInBlock::Transaction => tx,
            _ => {
                let with_upper = tx.bitor(MASK_5);
                with_upper.bitand(kind.mask())
            }
        })
    }
    /// Returns the transaction number. For extra-transaction appearances, this
    /// can be used to get the block number.
    pub fn get_tx_number(&self) -> TxNumber {
        self.0 & MASK_0
    }
    /// Returns the category of location in the block (transaction, miner, etc.)
    pub fn get_kind(&self) -> LocationInBlock {
        LocationInBlock::from_tx_number(&self.0)
    }
}

/// Refers a to a particular place in a block, either a field in a block object or a transaction
///
/// Used for encoding extra-transaction address appearances as transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationInBlock {
    /// Genesis object only.
    Alloc,
    /// Block "miners" field.
    Miner,
    /// An object in the "transactions" field.
    Transaction,
    /// Block "uncles" field.
    Uncles,
    /// Block "withdrawals" field.
    Withdrawals,
}

impl LocationInBlock {
    fn mask(&self) -> u64 {
        match self {
            LocationInBlock::Transaction => MASK_0,
            LocationInBlock::Alloc => MASK_1,
            LocationInBlock::Miner => MASK_2,
            LocationInBlock::Uncles => MASK_3,
            LocationInBlock::Withdrawals => MASK_4,
        }
    }

    fn from_tx_number(tx: &TxNumber) -> Self {
        match tx.bitor(MASK_0) {
            MASK_0 => LocationInBlock::Transaction,
            MASK_1 => LocationInBlock::Alloc,
            MASK_2 => LocationInBlock::Miner,
            MASK_3 => LocationInBlock::Uncles,
            MASK_4 => LocationInBlock::Withdrawals,
            _ => LocationInBlock::Transaction, // Could return Error here.
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn block_location_round_trip() {
        let first_tx: TxNumber = 123456789;
        let kind = LocationInBlock::Withdrawals;
        let withdrawal_as_tx = AddressLocation::new(first_tx, kind.clone());
        assert_eq!(first_tx, withdrawal_as_tx.get_tx_number());
        assert_eq!(kind, withdrawal_as_tx.get_kind());

        let kind = LocationInBlock::Miner;
        let withdrawal_as_tx = AddressLocation::new(first_tx, kind.clone());
        assert_eq!(first_tx, withdrawal_as_tx.get_tx_number());
        assert_eq!(kind, withdrawal_as_tx.get_kind());

        let kind = LocationInBlock::Uncles;
        let withdrawal_as_tx = AddressLocation::new(first_tx, kind.clone());
        assert_eq!(first_tx, withdrawal_as_tx.get_tx_number());
        assert_eq!(kind, withdrawal_as_tx.get_kind());

        let kind = LocationInBlock::Transaction;
        let withdrawal_as_tx = AddressLocation::new(first_tx, kind.clone());
        assert_eq!(first_tx, withdrawal_as_tx.get_tx_number());
        assert_eq!(kind, withdrawal_as_tx.get_kind());

        let kind = LocationInBlock::Alloc;
        let withdrawal_as_tx = AddressLocation::new(first_tx, kind.clone());
        assert_eq!(first_tx, withdrawal_as_tx.get_tx_number());
        assert_eq!(kind, withdrawal_as_tx.get_kind());
    }
}
