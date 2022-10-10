//! Internal helpers for testing.
#![allow(missing_docs)]

use crate::{PoolTransaction, TransactionOrdering};
use reth_primitives::{Address, TxHash, H256, U256};

/// Sets the value fo the field
macro_rules! set_value {
    ($this:ident => $field:ident) => {
        let new_value = $field;
        match $this {
            MockTransaction::Legacy { ref mut $field, .. } => {
                *$field = new_value;
            }
            MockTransaction::Eip1559 { ref mut $field, .. } => {
                *$field = new_value;
            }
        }
    };
}

/// Sets the value fo the field
macro_rules! get_value {
    ($this:ident => $field:ident) => {
        match $this {
            MockTransaction::Legacy { $field, .. } => $field,
            MockTransaction::Eip1559 { $field, .. } => $field,
        }
    };
}

/// A Bare transaction type used for testing.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum MockTransaction {
    Legacy {
        hash: H256,
        sender: Address,
        nonce: u64,
        gas_price: u64,
        gas_limit: u64,
        value: U256,
    },
    Eip1559 {
        hash: H256,
        sender: Address,
        nonce: u64,
        max_fee_per_gas: u64,
        max_priority_fee_per_gas: u64,
        gas_limit: U256,
        value: U256,
    },
}

// === impl MockTransaction ===

impl MockTransaction {
    pub fn set_nonce(&mut self, nonce: u64) -> &mut Self {
        set_value!(self => nonce);
        self
    }

    pub fn with_nonce(mut self, nonce: u64) -> Self {
        set_value!(self => nonce);
        self
    }

    pub fn get_nonce(&self) -> u64 {
        *get_value!(self => nonce)
    }

    /// Returns a clone with an increased nonce
    pub fn next(&self) -> Self {
        let mut next = self.clone();
        next.with_nonce(self.get_nonce())
    }
}

impl PoolTransaction for MockTransaction {
    fn hash(&self) -> &TxHash {
        match self {
            MockTransaction::Legacy { hash, .. } => hash,
            MockTransaction::Eip1559 { hash, .. } => hash,
        }
    }

    fn sender(&self) -> &Address {
        match self {
            MockTransaction::Legacy { sender, .. } => sender,
            MockTransaction::Eip1559 { sender, .. } => sender,
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            MockTransaction::Legacy { nonce, .. } => *nonce,
            MockTransaction::Eip1559 { nonce, .. } => *nonce,
        }
    }

    fn cost(&self) -> U256 {
        match self {
            MockTransaction::Legacy { gas_price, value, gas_limit, .. } => {
                U256::from(*gas_limit) * *gas_price + *value
            }
            MockTransaction::Eip1559 { max_fee_per_gas, value, gas_limit, .. } => {
                U256::from(*gas_limit) * *max_fee_per_gas + *value
            }
        }
    }

    fn max_fee_per_gas(&self) -> Option<u64> {
        match self {
            MockTransaction::Legacy { .. } => None,
            MockTransaction::Eip1559 { max_fee_per_gas, .. } => Some(*max_fee_per_gas),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u64> {
        match self {
            MockTransaction::Legacy { .. } => None,
            MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } => {
                Some(*max_priority_fee_per_gas)
            }
        }
    }
}

#[derive(Default)]
#[non_exhaustive]
struct MockOrdering;

impl TransactionOrdering for MockOrdering {
    type Priority = u64;
    type Transaction = MockTransaction;

    fn priority(&self, transaction: &Self::Transaction) -> Self::Priority {
        0
    }
}
