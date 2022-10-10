//! Internal helpers for testing.
#![allow(missing_docs)]

use crate::{PoolTransaction, TransactionOrdering};
use paste::paste;
use reth_primitives::{Address, TxHash, H256, U256};

/// Sets the value for the field
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

// Generates all setters and getters
macro_rules! make_setters_getters {
    ($($name:ident => $t:ty);*) => {
  paste! {
        $(
            pub fn [<set_ $name>](&mut self, $name: $t) -> &mut Self {
                set_value!(self => $name);
                self
            }

            pub fn [<with_$name>](mut self, $name: $t) -> Self {
                set_value!(self => $name);
                self
            }

            pub fn [<get_$name>](&self) -> $t {
                get_value!(self => $name).clone()
            }

        )*

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
        gas_limit: u64,
        value: U256,
    },
}

// === impl MockTransaction ===

impl MockTransaction {
    make_setters_getters! {
        nonce => u64;
        hash => H256;
        sender => Address;
        gas_limit => u64;
        value => U256
    }

    /// Returns a new legacy transaction with random address and hash and empty values
    fn legacy() -> Self {
        MockTransaction::Legacy {
            hash: H256::random(),
            sender: Address::random(),
            nonce: 0,
            gas_price: 0,
            gas_limit: 0,
            value: Default::default(),
        }
    }

    /// Returns a new EIP1559 transaction with random address and hash and empty values
    fn eip1559() -> Self {
        MockTransaction::Eip1559 {
            hash: H256::random(),
            sender: Address::random(),
            nonce: 0,
            max_fee_per_gas: 0,
            max_priority_fee_per_gas: 0,
            gas_limit: 0,
            value: Default::default(),
        }
    }

    pub fn set_priority_fee(&mut self, val: u64) -> &mut Self {
        if let MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } = self {
            *max_priority_fee_per_gas = val;
        }
        self
    }

    pub fn with_priority_fee(mut self, val: u64) -> Self {
        if let MockTransaction::Eip1559 { ref mut max_priority_fee_per_gas, .. } = self {
            *max_priority_fee_per_gas = val;
        }
        self
    }

    pub fn get_priority_fee(&self) -> Option<u64> {
        if let MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } = self {
            Some(*max_priority_fee_per_gas)
        } else {
            None
        }
    }

    pub fn set_max_fee(&mut self, val: u64) -> &mut Self {
        if let MockTransaction::Eip1559 { max_fee_per_gas, .. } = self {
            *max_fee_per_gas = val;
        }
        self
    }

    pub fn with_max_fee(mut self, val: u64) -> Self {
        if let MockTransaction::Eip1559 { ref mut max_fee_per_gas, .. } = self {
            *max_fee_per_gas = val;
        }
        self
    }

    pub fn get_max_fee(&self) -> Option<u64> {
        if let MockTransaction::Eip1559 { max_fee_per_gas, .. } = self {
            Some(*max_fee_per_gas)
        } else {
            None
        }
    }

    pub fn set_gas_price(&mut self, val: u64) -> &mut Self {
        match self {
            MockTransaction::Legacy { gas_price, .. } => {
                *gas_price = val;
            }
            MockTransaction::Eip1559 { max_fee_per_gas, max_priority_fee_per_gas, .. } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
        }
        self
    }

    pub fn with_gas_price(mut self, val: u64) -> Self {
        match self {
            MockTransaction::Legacy { ref mut gas_price, .. } => {
                *gas_price = val;
            }
            MockTransaction::Eip1559 {
                ref mut max_fee_per_gas,
                ref mut max_priority_fee_per_gas,
                ..
            } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
        }
        self
    }

    pub fn get_gas_price(&self) -> u64 {
        match self {
            MockTransaction::Legacy { gas_price, .. } => *gas_price,
            MockTransaction::Eip1559 { max_fee_per_gas, .. } => *max_fee_per_gas,
        }
    }

    /// Returns a clone with an increased nonce
    pub fn next(&self) -> Self {
        let mut next = self.clone();
        next.with_nonce(self.get_nonce() + 1)
    }
    /// Returns a clone with an increased nonce
    pub fn skip(&self, skip: u64) -> Self {
        let mut next = self.clone();
        next.with_nonce(self.get_nonce() + skip + 1)
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
