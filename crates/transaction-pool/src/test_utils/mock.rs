//! Mock Types

use crate::{
    identifier::{SenderIdentifiers, TransactionId},
    pool::txpool::TxPool,
    traits::TransactionOrigin,
    PoolTransaction, Priority, TransactionOrdering, ValidPoolTransaction,
};
use paste::paste;
use rand::{
    distributions::{Uniform, WeightedIndex},
    prelude::Distribution,
};
use reth_primitives::{
    constants::{eip4844::DATA_GAS_PER_BLOB, MIN_PROTOCOL_BASE_FEE},
    hex, AccessList, Address, BlobTransactionSidecar, Bytes, FromRecoveredPooledTransaction,
    FromRecoveredTransaction, IntoRecoveredTransaction, PooledTransactionsElementEcRecovered,
    Signature, Transaction, TransactionKind, TransactionSigned, TransactionSignedEcRecovered,
    TxEip1559, TxEip2930, TxEip4844, TxHash, TxLegacy, TxType, TxValue, B256, EIP1559_TX_TYPE_ID,
    EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, LEGACY_TX_TYPE_ID, U128, U256,
};
use std::{ops::Range, sync::Arc, time::Instant};

pub type MockTxPool = TxPool<MockOrdering>;

pub type MockValidTx = ValidPoolTransaction<MockTransaction>;

#[cfg(feature = "optimism")]
use reth_primitives::DEPOSIT_TX_TYPE_ID;

#[cfg(feature = "optimism")]
use reth_primitives::TxDeposit;

/// Create an empty `TxPool`
pub fn mock_tx_pool() -> MockTxPool {
    MockTxPool::new(Default::default(), Default::default())
}

macro_rules! op_set_value {
    ($this:ident, sender, $value:ident) => {
        $this.from = $value;
    };
    ($this:ident, gas_limit, $value:ident) => {
        $this.gas_limit = $value;
    };
    ($this:ident, value, $value:ident) => {
        $this.value = $value.into();
    };
    ($this:ident, input, $value:ident) => {
        $this.value = $value;
    };
    ($this:ident, $other:ident, $field:ident) => {};
}

macro_rules! op_get_value {
    ($this:ident, sender) => {
        $this.from
    };
    ($this:ident, gas_limit) => {
        $this.gas_limit
    };
    ($this:ident, value) => {
        $this.value.into()
    };
    ($this:ident, input) => {
        $this.input.clone()
    };
    ($this:ident, $other:ident) => {
        Default::default()
    };
}

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
            MockTransaction::Eip4844 { ref mut $field, .. } => {
                *$field = new_value;
            }
            MockTransaction::Eip2930 { ref mut $field, .. } => {
                *$field = new_value;
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(ref mut tx) => {
                op_set_value!(tx, $this, new_value);
            }
        }
    };
}

/// Gets the value for the field
macro_rules! get_value {
    ($this:tt => $field:ident) => {
        match $this {
            MockTransaction::Legacy { $field, .. } => $field.clone(),
            MockTransaction::Eip1559 { $field, .. } => $field.clone(),
            MockTransaction::Eip4844 { $field, .. } => $field.clone(),
            MockTransaction::Eip2930 { $field, .. } => $field.clone(),
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(tx) => {
                op_get_value!(tx, $field)
            }
        }
    };
}

// Generates all setters and getters
macro_rules! make_setters_getters {
    ($($name:ident => $t:ty);*) => {
        paste! {$(
            pub fn [<set_ $name>](&mut self, $name: $t) -> &mut Self {
                set_value!(self => $name);
                self
            }

            pub fn [<with_ $name>](mut self, $name: $t) -> Self {
                set_value!(self => $name);
                self
            }

            pub fn [<get_ $name>](&self) -> $t {
                get_value!(self => $name)
            }
        )*}
    };
}

/// A Bare transaction type used for testing.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum MockTransaction {
    Legacy {
        hash: B256,
        sender: Address,
        nonce: u64,
        gas_price: u128,
        gas_limit: u64,
        to: TransactionKind,
        value: U256,
        input: Bytes,
    },
    Eip1559 {
        hash: B256,
        sender: Address,
        nonce: u64,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        gas_limit: u64,
        to: TransactionKind,
        value: U256,
        accesslist: AccessList,
        input: Bytes,
    },
    Eip4844 {
        hash: B256,
        sender: Address,
        nonce: u64,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        max_fee_per_blob_gas: u128,
        gas_limit: u64,
        to: TransactionKind,
        value: U256,
        accesslist: AccessList,
        input: Bytes,
        sidecar: BlobTransactionSidecar,
    },
    Eip2930 {
        hash: B256,
        sender: Address,
        nonce: u64,
        to: TransactionKind,
        gas_limit: u64,
        input: Bytes,
        value: U256,
        gas_price: u128,
        accesslist: AccessList,
    },
    #[cfg(feature = "optimism")]
    Deposit(TxDeposit),
}

// === impl MockTransaction ===

impl MockTransaction {
    make_setters_getters! {
        nonce => u64;
        hash => B256;
        sender => Address;
        gas_limit => u64;
        value => U256;
        input => Bytes
    }

    /// Returns a new legacy transaction with random address and hash and empty values
    pub fn legacy() -> Self {
        MockTransaction::Legacy {
            hash: B256::random(),
            sender: Address::random(),
            nonce: 0,
            gas_price: 0,
            gas_limit: 0,
            to: TransactionKind::Call(Address::random()),
            value: Default::default(),
            input: Default::default(),
        }
    }

    /// Returns a new EIP1559 transaction with random address and hash and empty values
    pub fn eip1559() -> Self {
        MockTransaction::Eip1559 {
            hash: B256::random(),
            sender: Address::random(),
            nonce: 0,
            max_fee_per_gas: MIN_PROTOCOL_BASE_FEE as u128,
            max_priority_fee_per_gas: MIN_PROTOCOL_BASE_FEE as u128,
            gas_limit: 0,
            to: TransactionKind::Call(Address::random()),
            value: Default::default(),
            input: Bytes::new(),
            accesslist: Default::default(),
        }
    }

    /// Returns a new EIP4844 transaction with random address and hash and empty values
    pub fn eip4844() -> Self {
        MockTransaction::Eip4844 {
            hash: B256::random(),
            sender: Address::random(),
            nonce: 0,
            max_fee_per_gas: MIN_PROTOCOL_BASE_FEE as u128,
            max_priority_fee_per_gas: MIN_PROTOCOL_BASE_FEE as u128,
            max_fee_per_blob_gas: DATA_GAS_PER_BLOB as u128,
            gas_limit: 0,
            to: TransactionKind::Call(Address::random()),
            value: Default::default(),
            input: Bytes::new(),
            accesslist: Default::default(),
            sidecar: Default::default(),
        }
    }

    /// Returns a new EIP2930 transaction with random address and hash and empty values
    pub fn eip2930() -> Self {
        MockTransaction::Eip2930 {
            hash: B256::random(),
            sender: Address::random(),
            nonce: 0,
            to: TransactionKind::Call(Address::random()),
            gas_limit: 0,
            input: Bytes::new(),
            value: Default::default(),
            gas_price: 0,
            accesslist: Default::default(),
        }
    }

    /// Returns a new deposit transaction with random address and hash and empty values
    #[cfg(feature = "optimism")]
    pub fn deposit() -> Self {
        MockTransaction::Deposit(TxDeposit {
            source_hash: B256::random(),
            from: Address::random(),
            to: TransactionKind::Call(Address::random()),
            mint: Some(0),
            value: Default::default(),
            gas_limit: 0,
            is_system_transaction: false,
            input: Bytes::new(),
        })
    }

    /// Creates a new transaction with the given [TxType].
    ///
    /// See the default constructors for each of the transaction types:
    ///
    /// * [MockTransaction::legacy]
    /// * [MockTransaction::eip2930]
    /// * [MockTransaction::eip1559]
    /// * [MockTransaction::eip4844]
    pub fn new_from_type(tx_type: TxType) -> Self {
        match tx_type {
            TxType::Legacy => Self::legacy(),
            TxType::EIP2930 => Self::eip2930(),
            TxType::EIP1559 => Self::eip1559(),
            TxType::EIP4844 => Self::eip4844(),
            #[cfg(feature = "optimism")]
            TxType::DEPOSIT => Self::deposit(),
        }
    }

    /// Sets the max fee per blob gas for EIP-4844 transactions,
    pub fn with_blob_fee(mut self, val: u128) -> Self {
        self.set_blob_fee(val);
        self
    }

    /// Sets the max fee per blob gas for EIP-4844 transactions,
    pub fn set_blob_fee(&mut self, val: u128) -> &mut Self {
        if let MockTransaction::Eip4844 { max_fee_per_blob_gas, .. } = self {
            *max_fee_per_blob_gas = val;
        }
        self
    }

    /// Sets the priority fee for dynamic fee transactions (EIP-1559 and EIP-4844)
    pub fn set_priority_fee(&mut self, val: u128) -> &mut Self {
        if let (MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } |
        MockTransaction::Eip4844 { max_priority_fee_per_gas, .. }) = self
        {
            *max_priority_fee_per_gas = val;
        }
        self
    }

    pub fn with_priority_fee(mut self, val: u128) -> Self {
        self.set_priority_fee(val);
        self
    }

    pub fn get_priority_fee(&self) -> Option<u128> {
        if let (MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } |
        MockTransaction::Eip4844 { max_priority_fee_per_gas, .. }) = self
        {
            Some(*max_priority_fee_per_gas)
        } else {
            None
        }
    }

    pub fn set_max_fee(&mut self, val: u128) -> &mut Self {
        if let (MockTransaction::Eip1559 { max_fee_per_gas, .. } |
        MockTransaction::Eip4844 { max_fee_per_gas, .. }) = self
        {
            *max_fee_per_gas = val;
        }
        self
    }

    pub fn with_max_fee(mut self, val: u128) -> Self {
        self.set_max_fee(val);
        self
    }

    pub fn get_max_fee(&self) -> Option<u128> {
        if let (MockTransaction::Eip1559 { max_fee_per_gas, .. } |
        MockTransaction::Eip4844 { max_fee_per_gas, .. }) = self
        {
            Some(*max_fee_per_gas)
        } else {
            None
        }
    }

    pub fn set_accesslist(&mut self, list: AccessList) -> &mut Self {
        match self {
            MockTransaction::Legacy { .. } => {}
            MockTransaction::Eip1559 { accesslist, .. } => {
                *accesslist = list;
            }
            MockTransaction::Eip4844 { accesslist, .. } => {
                *accesslist = list;
            }
            MockTransaction::Eip2930 { accesslist, .. } => {
                *accesslist = list;
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit { .. } => {}
        }
        self
    }

    pub fn set_gas_price(&mut self, val: u128) -> &mut Self {
        match self {
            MockTransaction::Legacy { gas_price, .. } => {
                *gas_price = val;
            }
            MockTransaction::Eip1559 { max_fee_per_gas, max_priority_fee_per_gas, .. } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
            MockTransaction::Eip4844 { max_fee_per_gas, max_priority_fee_per_gas, .. } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
            MockTransaction::Eip2930 { gas_price, .. } => *gas_price = val,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => {}
        }
        self
    }

    pub fn with_gas_price(mut self, val: u128) -> Self {
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
            MockTransaction::Eip4844 {
                ref mut max_fee_per_gas,
                ref mut max_priority_fee_per_gas,
                ..
            } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
            MockTransaction::Eip2930 { ref mut gas_price, .. } => {
                *gas_price = val;
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => {}
        }
        self
    }

    pub fn get_gas_price(&self) -> u128 {
        match self {
            MockTransaction::Legacy { gas_price, .. } |
            MockTransaction::Eip2930 { gas_price, .. } => *gas_price,
            MockTransaction::Eip1559 { max_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_fee_per_gas, .. } => *max_fee_per_gas,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u128,
        }
    }

    /// Returns a clone with a decreased nonce
    pub fn prev(&self) -> Self {
        let mut next = self.clone().with_hash(B256::random());
        next.with_nonce(self.get_nonce() - 1)
    }

    /// Returns a clone with an increased nonce
    pub fn next(&self) -> Self {
        let mut next = self.clone().with_hash(B256::random());
        next.with_nonce(self.get_nonce() + 1)
    }

    /// Returns a clone with an increased nonce
    pub fn skip(&self, skip: u64) -> Self {
        let mut next = self.clone().with_hash(B256::random());
        next.with_nonce(self.get_nonce() + skip + 1)
    }

    /// Returns a clone with incremented nonce
    pub fn inc_nonce(mut self) -> Self {
        let nonce = self.get_nonce() + 1;
        self.with_nonce(nonce)
    }

    /// Sets a new random hash
    pub fn rng_hash(mut self) -> Self {
        self.with_hash(B256::random())
    }

    /// Returns a new transaction with a higher gas price +1
    pub fn inc_price(&self) -> Self {
        self.inc_price_by(1)
    }

    /// Returns a new transaction with a higher gas price
    pub fn inc_price_by(&self, value: u128) -> Self {
        let mut next = self.clone();
        let gas = self.get_gas_price().checked_add(value).unwrap();
        next.with_gas_price(gas)
    }

    /// Returns a new transaction with a lower gas price -1
    pub fn decr_price(&self) -> Self {
        self.decr_price_by(1)
    }

    /// Returns a new transaction with a lower gas price
    pub fn decr_price_by(&self, value: u128) -> Self {
        let mut next = self.clone();
        let gas = self.get_gas_price().checked_sub(value).unwrap();
        next.with_gas_price(gas)
    }

    /// Returns a new transaction with a higher value
    pub fn inc_value(&self) -> Self {
        let mut next = self.clone();
        let val = self.get_value().checked_add(U256::from(1)).unwrap();
        next.with_value(val)
    }

    /// Returns a new transaction with a higher gas limit
    pub fn inc_limit(&self) -> Self {
        let mut next = self.clone();
        let gas = self.get_gas_limit() + 1;
        next.with_gas_limit(gas)
    }

    pub fn tx_type(&self) -> u8 {
        match self {
            Self::Legacy { .. } => LEGACY_TX_TYPE_ID,
            Self::Eip1559 { .. } => EIP1559_TX_TYPE_ID,
            Self::Eip4844 { .. } => EIP4844_TX_TYPE_ID,
            Self::Eip2930 { .. } => EIP2930_TX_TYPE_ID,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => DEPOSIT_TX_TYPE_ID,
        }
    }

    pub fn is_legacy(&self) -> bool {
        matches!(self, MockTransaction::Legacy { .. })
    }

    pub fn is_eip1559(&self) -> bool {
        matches!(self, MockTransaction::Eip1559 { .. })
    }

    pub fn is_eip4844(&self) -> bool {
        matches!(self, MockTransaction::Eip4844 { .. })
    }

    pub fn is_eip2930(&self) -> bool {
        matches!(self, MockTransaction::Eip2930 { .. })
    }
}

impl PoolTransaction for MockTransaction {
    fn hash(&self) -> &TxHash {
        match self {
            MockTransaction::Legacy { hash, .. } => hash,
            MockTransaction::Eip1559 { hash, .. } => hash,
            MockTransaction::Eip4844 { hash, .. } => hash,
            MockTransaction::Eip2930 { hash, .. } => hash,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(TxDeposit { source_hash, .. }) => source_hash,
        }
    }

    fn sender(&self) -> Address {
        match self {
            MockTransaction::Legacy { sender, .. } => *sender,
            MockTransaction::Eip1559 { sender, .. } => *sender,
            MockTransaction::Eip4844 { sender, .. } => *sender,
            MockTransaction::Eip2930 { sender, .. } => *sender,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(TxDeposit { from, .. }) => *from,
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            MockTransaction::Legacy { nonce, .. } => *nonce,
            MockTransaction::Eip1559 { nonce, .. } => *nonce,
            MockTransaction::Eip4844 { nonce, .. } => *nonce,
            MockTransaction::Eip2930 { nonce, .. } => *nonce,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u64,
        }
    }

    fn cost(&self) -> U256 {
        match self {
            MockTransaction::Legacy { gas_price, value, gas_limit, .. } => {
                U256::from(*gas_limit) * U256::from(*gas_price) + *value
            }
            MockTransaction::Eip1559 { max_fee_per_gas, value, gas_limit, .. } => {
                U256::from(*gas_limit) * U256::from(*max_fee_per_gas) + *value
            }
            MockTransaction::Eip4844 { max_fee_per_gas, value, gas_limit, .. } => {
                U256::from(*gas_limit) * U256::from(*max_fee_per_gas) + *value
            }
            MockTransaction::Eip2930 { gas_limit, gas_price, value, .. } => {
                U256::from(*gas_limit) * U256::from(*gas_price) + *value
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => U256::ZERO,
        }
    }

    fn gas_limit(&self) -> u64 {
        self.get_gas_limit()
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            MockTransaction::Legacy { gas_price, .. } => *gas_price,
            MockTransaction::Eip1559 { max_fee_per_gas, .. } => *max_fee_per_gas,
            MockTransaction::Eip4844 { max_fee_per_gas, .. } => *max_fee_per_gas,
            MockTransaction::Eip2930 { gas_price, .. } => *gas_price,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u128,
        }
    }

    fn access_list(&self) -> Option<&AccessList> {
        match self {
            MockTransaction::Legacy { .. } => None,
            MockTransaction::Eip1559 { accesslist: accessslist, .. } => Some(accessslist),
            MockTransaction::Eip4844 { accesslist: accessslist, .. } => Some(accessslist),
            MockTransaction::Eip2930 { accesslist, .. } => Some(accesslist),
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => None,
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            MockTransaction::Legacy { .. } => None,
            MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } => {
                Some(*max_priority_fee_per_gas)
            }
            MockTransaction::Eip4844 { max_priority_fee_per_gas, .. } => {
                Some(*max_priority_fee_per_gas)
            }
            MockTransaction::Eip2930 { .. } => None,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => None,
        }
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            MockTransaction::Eip4844 { max_fee_per_blob_gas, .. } => Some(*max_fee_per_blob_gas),
            _ => None,
        }
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        let base_fee = base_fee as u128;
        let max_fee_per_gas = self.max_fee_per_gas();
        if max_fee_per_gas < base_fee {
            return None
        }

        let fee = max_fee_per_gas - base_fee;
        if let Some(priority_fee) = self.max_priority_fee_per_gas() {
            return Some(fee.min(priority_fee))
        }

        Some(fee)
    }

    fn priority_fee_or_price(&self) -> u128 {
        match self {
            MockTransaction::Legacy { gas_price, .. } => *gas_price,
            MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } => *max_priority_fee_per_gas,
            MockTransaction::Eip4844 { max_priority_fee_per_gas, .. } => *max_priority_fee_per_gas,
            MockTransaction::Eip2930 { gas_price, .. } => *gas_price,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u128,
        }
    }

    fn kind(&self) -> &TransactionKind {
        match self {
            MockTransaction::Legacy { to, .. } => to,
            MockTransaction::Eip1559 { to, .. } => to,
            MockTransaction::Eip4844 { to, .. } => to,
            MockTransaction::Eip2930 { to, .. } => to,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(TxDeposit { to, .. }) => to,
        }
    }

    fn input(&self) -> &[u8] {
        match self {
            MockTransaction::Legacy { .. } => &[],
            MockTransaction::Eip1559 { input, .. } => input,
            MockTransaction::Eip4844 { input, .. } => input,
            MockTransaction::Eip2930 { input, .. } => input,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit { .. } => &[],
        }
    }

    fn size(&self) -> usize {
        0
    }

    fn tx_type(&self) -> u8 {
        match self {
            MockTransaction::Legacy { .. } => TxType::Legacy.into(),
            MockTransaction::Eip1559 { .. } => TxType::EIP1559.into(),
            MockTransaction::Eip4844 { .. } => TxType::EIP4844.into(),
            MockTransaction::Eip2930 { .. } => TxType::EIP2930.into(),
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => DEPOSIT_TX_TYPE_ID,
        }
    }

    fn encoded_length(&self) -> usize {
        0
    }

    fn chain_id(&self) -> Option<u64> {
        Some(1)
    }

    #[cfg(feature = "optimism")]
    fn is_deposit(&self) -> bool {
        matches!(self, MockTransaction::Deposit(_))
    }
}

impl FromRecoveredTransaction for MockTransaction {
    fn from_recovered_transaction(tx: TransactionSignedEcRecovered) -> Self {
        let sender = tx.signer();
        let transaction = tx.into_signed();
        let hash = transaction.hash();
        match transaction.transaction {
            Transaction::Legacy(TxLegacy {
                chain_id: _,
                nonce,
                gas_price,
                gas_limit,
                to,
                value,
                input,
            }) => MockTransaction::Legacy {
                hash,
                sender,
                nonce,
                gas_price,
                gas_limit,
                to,
                value: value.into(),
                input,
            },
            Transaction::Eip1559(TxEip1559 {
                chain_id: _,
                nonce,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to,
                value,
                input,
                access_list,
            }) => MockTransaction::Eip1559 {
                hash,
                sender,
                nonce,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                gas_limit,
                to,
                value: value.into(),
                input,
                accesslist: access_list,
            },
            Transaction::Eip4844(TxEip4844 {
                chain_id: _,
                nonce,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to,
                value,
                input,
                access_list,
                blob_versioned_hashes: _,
                max_fee_per_blob_gas,
            }) => MockTransaction::Eip4844 {
                hash,
                sender,
                nonce,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                max_fee_per_blob_gas,
                gas_limit,
                to,
                value: value.into(),
                input,
                accesslist: access_list,
                sidecar: BlobTransactionSidecar::default(),
            },
            Transaction::Eip2930(TxEip2930 {
                chain_id: _,
                nonce,
                gas_price,
                gas_limit,
                to,
                value,
                input,
                access_list,
            }) => MockTransaction::Eip2930 {
                hash,
                sender,
                nonce,
                gas_price,
                gas_limit,
                to,
                value: value.into(),
                input,
                accesslist: access_list,
            },
            #[cfg(feature = "optimism")]
            Transaction::Deposit(TxDeposit {
                source_hash,
                from,
                to,
                mint,
                value,
                gas_limit,
                is_system_transaction,
                input,
            }) => MockTransaction::Deposit(TxDeposit {
                source_hash,
                from,
                to,
                mint,
                value,
                gas_limit,
                is_system_transaction,
                input,
            }),
        }
    }
}

impl FromRecoveredPooledTransaction for MockTransaction {
    fn from_recovered_pooled_transaction(tx: PooledTransactionsElementEcRecovered) -> Self {
        FromRecoveredTransaction::from_recovered_transaction(tx.into_ecrecovered_transaction())
    }
}

impl IntoRecoveredTransaction for MockTransaction {
    fn to_recovered_transaction(&self) -> TransactionSignedEcRecovered {
        let tx = self.clone().into();

        let signed_tx = TransactionSigned {
            hash: *self.hash(),
            signature: Signature::default(),
            transaction: tx,
        };

        TransactionSignedEcRecovered::from_signed_transaction(signed_tx, self.sender())
    }
}

impl From<MockTransaction> for Transaction {
    fn from(mock: MockTransaction) -> Self {
        match mock {
            MockTransaction::Legacy {
                hash,
                sender,
                nonce,
                gas_price,
                gas_limit,
                to,
                value,
                input,
            } => Self::Legacy(TxLegacy {
                chain_id: Some(1),
                nonce,
                gas_price,
                gas_limit,
                to,
                value: value.into(),
                input: input.clone(),
            }),
            MockTransaction::Eip1559 {
                hash,
                sender,
                nonce,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                gas_limit,
                to,
                value,
                accesslist,
                input,
            } => Self::Eip1559(TxEip1559 {
                chain_id: 1,
                nonce,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to,
                value: value.into(),
                access_list: accesslist.clone(),
                input: input.clone(),
            }),
            MockTransaction::Eip4844 {
                hash,
                sender,
                nonce,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                max_fee_per_blob_gas,
                gas_limit,
                to,
                value,
                accesslist,
                input,
                sidecar,
            } => Self::Eip4844(TxEip4844 {
                chain_id: 1,
                nonce,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to,
                value: value.into(),
                access_list: accesslist,
                blob_versioned_hashes: vec![hash],
                max_fee_per_blob_gas,
                input,
            }),
            MockTransaction::Eip2930 {
                hash,
                sender,
                nonce,
                to,
                gas_limit,
                input,
                value,
                gas_price,
                accesslist,
            } => Self::Eip2930(TxEip2930 {
                chain_id: 1,
                nonce,
                gas_price,
                gas_limit,
                to,
                value: value.into(),
                access_list: accesslist,
                input,
            }),
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(tx) => Self::Deposit(tx),
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for MockTransaction {
    type Parameters = ();
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::{any, Strategy};

        any::<(Transaction, Address, B256)>()
            .prop_map(|(tx, sender, tx_hash)| match &tx {
                Transaction::Legacy(TxLegacy {
                    nonce,
                    gas_price,
                    gas_limit,
                    to,
                    value,
                    input,
                    ..
                }) |
                Transaction::Eip2930(TxEip2930 {
                    nonce,
                    gas_price,
                    gas_limit,
                    to,
                    value,
                    input,
                    ..
                }) => MockTransaction::Legacy {
                    sender,
                    hash: tx_hash,
                    nonce: *nonce,
                    gas_price: *gas_price,
                    gas_limit: *gas_limit,
                    to: *to,
                    value: (*value).into(),
                    input: (*input).clone(),
                },
                Transaction::Eip1559(TxEip1559 {
                    nonce,
                    gas_limit,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    to,
                    value,
                    input,
                    access_list,
                    ..
                }) => MockTransaction::Eip1559 {
                    sender,
                    hash: tx_hash,
                    nonce: *nonce,
                    max_fee_per_gas: *max_fee_per_gas,
                    max_priority_fee_per_gas: *max_priority_fee_per_gas,
                    gas_limit: *gas_limit,
                    to: *to,
                    value: (*value).into(),
                    input: (*input).clone(),
                    accesslist: (*access_list).clone(),
                },
                Transaction::Eip4844(TxEip4844 {
                    nonce,
                    gas_limit,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    to,
                    value,
                    input,
                    max_fee_per_blob_gas,
                    access_list,
                    ..
                }) => MockTransaction::Eip4844 {
                    sender,
                    hash: tx_hash,
                    nonce: *nonce,
                    max_fee_per_gas: *max_fee_per_gas,
                    max_priority_fee_per_gas: *max_priority_fee_per_gas,
                    max_fee_per_blob_gas: *max_fee_per_blob_gas,
                    gas_limit: *gas_limit,
                    to: *to,
                    value: (*value).into(),
                    input: (*input).clone(),
                    accesslist: (*access_list).clone(),
                    // only generate a sidecar if it is a 4844 tx - also for the sake of
                    // performance just use a default sidecar
                    sidecar: BlobTransactionSidecar::default(),
                },
                _ => unimplemented!(),
            })
            .boxed()
    }

    type Strategy = proptest::strategy::BoxedStrategy<MockTransaction>;
}

#[derive(Default)]
pub struct MockTransactionFactory {
    pub(crate) ids: SenderIdentifiers,
}

// === impl MockTransactionFactory ===

impl MockTransactionFactory {
    pub fn tx_id(&mut self, tx: &MockTransaction) -> TransactionId {
        let sender = self.ids.sender_id_or_create(tx.get_sender());
        TransactionId::new(sender, tx.get_nonce())
    }

    pub fn validated(&mut self, transaction: MockTransaction) -> MockValidTx {
        self.validated_with_origin(TransactionOrigin::External, transaction)
    }

    pub fn validated_arc(&mut self, transaction: MockTransaction) -> Arc<MockValidTx> {
        Arc::new(self.validated(transaction))
    }

    /// Converts the transaction into a validated transaction
    pub fn validated_with_origin(
        &mut self,
        origin: TransactionOrigin,
        transaction: MockTransaction,
    ) -> MockValidTx {
        let transaction_id = self.tx_id(&transaction);
        MockValidTx {
            propagate: false,
            transaction_id,
            transaction,
            timestamp: Instant::now(),
            origin,
        }
    }

    pub fn create_legacy(&mut self) -> MockValidTx {
        self.validated(MockTransaction::legacy())
    }

    pub fn create_eip1559(&mut self) -> MockValidTx {
        self.validated(MockTransaction::eip1559())
    }

    pub fn create_eip4844(&mut self) -> MockValidTx {
        self.validated(MockTransaction::eip4844())
    }
}

#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct MockOrdering;

impl TransactionOrdering for MockOrdering {
    type PriorityValue = U256;
    type Transaction = MockTransaction;

    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue> {
        transaction.effective_tip_per_gas(base_fee).map(U256::from).into()
    }
}

/// A configured distribution that can generate transactions
pub struct MockTransactionDistribution {
    /// legacy to EIP-1559 ration
    legacy_ratio: WeightedIndex<u32>,
    /// generates the gas limit
    gas_limit_range: Uniform<u64>,
}

impl MockTransactionDistribution {
    /// Creates a new generator distribution.
    ///
    /// Expects legacy tx in full pct: `30u32` is `30%`.
    pub fn new(legacy_pct: u32, gas_limit_range: Range<u64>) -> Self {
        assert!(legacy_pct <= 100, "expect pct");

        let eip_1559 = 100 - legacy_pct;
        Self {
            legacy_ratio: WeightedIndex::new([eip_1559, legacy_pct]).unwrap(),
            gas_limit_range: gas_limit_range.into(),
        }
    }

    /// Generates a new transaction
    pub fn tx(&self, nonce: u64, rng: &mut impl rand::Rng) -> MockTransaction {
        let tx = if self.legacy_ratio.sample(rng) == 0 {
            MockTransaction::eip1559()
        } else {
            MockTransaction::legacy()
        };
        tx.with_nonce(nonce).with_gas_limit(self.gas_limit_range.sample(rng))
    }
}

/// A set of [MockTransaction]s that can be modified at once
#[derive(Debug, Clone)]
pub struct MockTransactionSet {
    pub(crate) transactions: Vec<MockTransaction>,
}

impl MockTransactionSet {
    /// Create a new [MockTransactionSet] from a list of transactions
    fn new(transactions: Vec<MockTransaction>) -> Self {
        Self { transactions }
    }

    /// Create a list of dependent transactions with a common sender. The transactions start at the
    /// given nonce, and the sender is incremented by the given tx_count.
    pub fn dependent(sender: Address, from_nonce: u64, tx_count: usize, tx_type: TxType) -> Self {
        let mut txs = Vec::with_capacity(tx_count);
        let mut curr_tx = MockTransaction::new_from_type(tx_type).with_nonce(from_nonce);
        for i in 0..tx_count {
            let nonce = from_nonce + i as u64;
            curr_tx = curr_tx.next().with_sender(sender);
            txs.push(curr_tx.clone());
        }

        MockTransactionSet::new(txs)
    }

    /// Add transactions to the [MockTransactionSet]
    pub fn extend<T: IntoIterator<Item = MockTransaction>>(&mut self, txs: T) {
        self.transactions.extend(txs);
    }

    /// Extract the inner [Vec] of [MockTransaction]s
    pub fn into_vec(self) -> Vec<MockTransaction> {
        self.transactions
    }
}

#[test]
fn test_mock_priority() {
    let o = MockOrdering;
    let lo = MockTransaction::eip1559().with_gas_limit(100_000);
    let hi = lo.next().inc_price();
    assert!(o.priority(&hi, 0) > o.priority(&lo, 0));
}
