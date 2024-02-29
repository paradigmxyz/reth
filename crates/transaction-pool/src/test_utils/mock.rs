//! Mock types.

use crate::{
    identifier::{SenderIdentifiers, TransactionId},
    pool::txpool::TxPool,
    traits::TransactionOrigin,
    CoinbaseTipOrdering, PoolTransaction, ValidPoolTransaction,
};
use paste::paste;
use rand::{
    distributions::{Uniform, WeightedIndex},
    prelude::Distribution,
};
use reth_primitives::{
    constants::{eip4844::DATA_GAS_PER_BLOB, MIN_PROTOCOL_BASE_FEE},
    AccessList, Address, BlobTransactionSidecar, Bytes, FromRecoveredPooledTransaction,
    FromRecoveredTransaction, IntoRecoveredTransaction, PooledTransactionsElementEcRecovered,
    Signature, Transaction, TransactionKind, TransactionSigned, TransactionSignedEcRecovered,
    TxEip1559, TxEip2930, TxEip4844, TxHash, TxLegacy, TxType, B256, EIP1559_TX_TYPE_ID,
    EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, LEGACY_TX_TYPE_ID, U256,
};
use std::{ops::Range, sync::Arc, time::Instant, vec::IntoIter};

/// A transaction pool implementation using [MockOrdering] for transaction ordering.
///
/// This type is an alias for [`TxPool<MockOrdering>`].
pub type MockTxPool = TxPool<MockOrdering>;

/// A validated transaction in the transaction pool, using [MockTransaction] as the transaction
/// type.
///
/// This type is an alias for [`ValidPoolTransaction<MockTransaction>`].
pub type MockValidTx = ValidPoolTransaction<MockTransaction>;

#[cfg(feature = "optimism")]
use reth_primitives::DEPOSIT_TX_TYPE_ID;

#[cfg(feature = "optimism")]
use reth_primitives::TxDeposit;

/// Create an empty `TxPool`
pub fn mock_tx_pool() -> MockTxPool {
    MockTxPool::new(Default::default(), Default::default())
}

#[cfg(feature = "optimism")]
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

#[cfg(feature = "optimism")]
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
            MockTransaction::Legacy { ref mut $field, .. } |
            MockTransaction::Eip1559 { ref mut $field, .. } |
            MockTransaction::Eip4844 { ref mut $field, .. } |
            MockTransaction::Eip2930 { ref mut $field, .. } => {
                *$field = new_value;
            }
            #[cfg(feature = "optimism")]
            #[allow(unused_variables)]
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
            MockTransaction::Legacy { $field, .. } |
            MockTransaction::Eip1559 { $field, .. } |
            MockTransaction::Eip4844 { $field, .. } |
            MockTransaction::Eip2930 { $field, .. } => $field.clone(),
            #[cfg(feature = "optimism")]
            #[allow(unused_variables)]
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
            /// Sets the value of the specified field.
            pub fn [<set_ $name>](&mut self, $name: $t) -> &mut Self {
                set_value!(self => $name);
                self
            }

            /// Sets the value of the specified field using a fluent interface.
            pub fn [<with_ $name>](mut self, $name: $t) -> Self {
                set_value!(self => $name);
                self
            }

            /// Gets the value of the specified field.
            pub fn [<get_ $name>](&self) -> $t {
                get_value!(self => $name)
            }
        )*}
    };
}

/// A Bare transaction type used for testing.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum MockTransaction {
    /// Legacy transaction type.
    Legacy {
        /// The hash of the transaction.
        hash: B256,
        /// The sender's address.
        sender: Address,
        /// The transaction nonce.
        nonce: u64,
        /// The gas price for the transaction.
        gas_price: u128,
        /// The gas limit for the transaction.
        gas_limit: u64,
        /// The transaction's destination.
        to: TransactionKind,
        /// The value of the transaction.
        value: U256,
        /// The transaction input data.
        input: Bytes,
        /// The size of the transaction, returned in the implementation of [PoolTransaction].
        size: usize,
    },
    /// EIP-1559 transaction type.
    Eip1559 {
        /// The hash of the transaction.
        hash: B256,
        /// The sender's address.
        sender: Address,
        /// The transaction nonce.
        nonce: u64,
        /// The maximum fee per gas for the transaction.
        max_fee_per_gas: u128,
        /// The maximum priority fee per gas for the transaction.
        max_priority_fee_per_gas: u128,
        /// The gas limit for the transaction.
        gas_limit: u64,
        /// The transaction's destination.
        to: TransactionKind,
        /// The value of the transaction.
        value: U256,
        /// The access list associated with the transaction.
        accesslist: AccessList,
        /// The transaction input data.
        input: Bytes,
        /// The size of the transaction, returned in the implementation of [PoolTransaction].
        size: usize,
    },
    /// EIP-4844 transaction type.
    Eip4844 {
        /// The hash of the transaction.
        hash: B256,
        /// The sender's address.
        sender: Address,
        /// The transaction nonce.
        nonce: u64,
        /// The maximum fee per gas for the transaction.
        max_fee_per_gas: u128,
        /// The maximum priority fee per gas for the transaction.
        max_priority_fee_per_gas: u128,
        /// The maximum fee per blob gas for the transaction.
        max_fee_per_blob_gas: u128,
        /// The gas limit for the transaction.
        gas_limit: u64,
        /// The transaction's destination.
        to: TransactionKind,
        /// The value of the transaction.
        value: U256,
        /// The access list associated with the transaction.
        accesslist: AccessList,
        /// The transaction input data.
        input: Bytes,
        /// The sidecar information for the transaction.
        sidecar: BlobTransactionSidecar,
        /// The size of the transaction, returned in the implementation of [PoolTransaction].
        size: usize,
    },
    /// EIP-2930 transaction type.
    Eip2930 {
        /// The hash of the transaction.
        hash: B256,
        /// The sender's address.
        sender: Address,
        /// The transaction nonce.
        nonce: u64,
        /// The transaction's destination.
        to: TransactionKind,
        /// The gas limit for the transaction.
        gas_limit: u64,
        /// The transaction input data.
        input: Bytes,
        /// The value of the transaction.
        value: U256,
        /// The gas price for the transaction.
        gas_price: u128,
        /// The access list associated with the transaction.
        accesslist: AccessList,
        /// The size of the transaction, returned in the implementation of [PoolTransaction].
        size: usize,
    },
    #[cfg(feature = "optimism")]
    /// Deposit transaction type (Optimism feature).
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
        input => Bytes;
        size => usize
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
            size: Default::default(),
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
            size: Default::default(),
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
            size: Default::default(),
        }
    }

    /// Returns a new EIP4844 transaction with a provided sidecar
    pub fn eip4844_with_sidecar(sidecar: BlobTransactionSidecar) -> Self {
        let mut transaction = Self::eip4844();
        if let MockTransaction::Eip4844 { sidecar: ref mut existing_sidecar, .. } = &mut transaction
        {
            *existing_sidecar = sidecar;
        }
        transaction
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
            size: Default::default(),
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
        if let MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } |
        MockTransaction::Eip4844 { max_priority_fee_per_gas, .. } = self
        {
            *max_priority_fee_per_gas = val;
        }
        self
    }

    /// Sets the priority fee for dynamic fee transactions (EIP-1559 and EIP-4844)
    pub fn with_priority_fee(mut self, val: u128) -> Self {
        self.set_priority_fee(val);
        self
    }

    /// Gets the priority fee for dynamic fee transactions (EIP-1559 and EIP-4844)
    pub const fn get_priority_fee(&self) -> Option<u128> {
        match self {
            MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_priority_fee_per_gas, .. } => {
                Some(*max_priority_fee_per_gas)
            }
            _ => None,
        }
    }

    /// Sets the max fee for dynamic fee transactions (EIP-1559 and EIP-4844)
    pub fn set_max_fee(&mut self, val: u128) -> &mut Self {
        if let MockTransaction::Eip1559 { max_fee_per_gas, .. } |
        MockTransaction::Eip4844 { max_fee_per_gas, .. } = self
        {
            *max_fee_per_gas = val;
        }
        self
    }

    /// Sets the max fee for dynamic fee transactions (EIP-1559 and EIP-4844)
    pub fn with_max_fee(mut self, val: u128) -> Self {
        self.set_max_fee(val);
        self
    }

    /// Gets the max fee for dynamic fee transactions (EIP-1559 and EIP-4844)
    pub const fn get_max_fee(&self) -> Option<u128> {
        match self {
            MockTransaction::Eip1559 { max_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_fee_per_gas, .. } => Some(*max_fee_per_gas),
            _ => None,
        }
    }

    /// Sets the access list for transactions supporting EIP-1559, EIP-4844, and EIP-2930.
    pub fn set_accesslist(&mut self, list: AccessList) -> &mut Self {
        match self {
            MockTransaction::Legacy { .. } => {}
            MockTransaction::Eip1559 { accesslist, .. } |
            MockTransaction::Eip4844 { accesslist, .. } |
            MockTransaction::Eip2930 { accesslist, .. } => {
                *accesslist = list;
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit { .. } => {}
        }
        self
    }

    /// Sets the gas price for the transaction.
    pub fn set_gas_price(&mut self, val: u128) -> &mut Self {
        match self {
            MockTransaction::Legacy { gas_price, .. } |
            MockTransaction::Eip2930 { gas_price, .. } => {
                *gas_price = val;
            }
            MockTransaction::Eip1559 { max_fee_per_gas, max_priority_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_fee_per_gas, max_priority_fee_per_gas, .. } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => {}
        }
        self
    }

    /// Sets the gas price for the transaction.
    pub fn with_gas_price(mut self, val: u128) -> Self {
        match self {
            MockTransaction::Legacy { ref mut gas_price, .. } |
            MockTransaction::Eip2930 { ref mut gas_price, .. } => {
                *gas_price = val;
            }
            MockTransaction::Eip1559 {
                ref mut max_fee_per_gas,
                ref mut max_priority_fee_per_gas,
                ..
            } |
            MockTransaction::Eip4844 {
                ref mut max_fee_per_gas,
                ref mut max_priority_fee_per_gas,
                ..
            } => {
                *max_fee_per_gas = val;
                *max_priority_fee_per_gas = val;
            }
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => {}
        }
        self
    }

    /// Gets the gas price for the transaction.
    pub const fn get_gas_price(&self) -> u128 {
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
        self.clone().with_hash(B256::random()).with_nonce(self.get_nonce() - 1)
    }

    /// Returns a clone with an increased nonce
    pub fn next(&self) -> Self {
        self.clone().with_hash(B256::random()).with_nonce(self.get_nonce() + 1)
    }

    /// Returns a clone with an increased nonce
    pub fn skip(&self, skip: u64) -> Self {
        self.clone().with_hash(B256::random()).with_nonce(self.get_nonce() + skip + 1)
    }

    /// Returns a clone with incremented nonce
    pub fn inc_nonce(self) -> Self {
        let nonce = self.get_nonce() + 1;
        self.with_nonce(nonce)
    }

    /// Sets a new random hash
    pub fn rng_hash(self) -> Self {
        self.with_hash(B256::random())
    }

    /// Returns a new transaction with a higher gas price +1
    pub fn inc_price(&self) -> Self {
        self.inc_price_by(1)
    }

    /// Returns a new transaction with a higher gas price
    pub fn inc_price_by(&self, value: u128) -> Self {
        self.clone().with_gas_price(self.get_gas_price().checked_add(value).unwrap())
    }

    /// Returns a new transaction with a lower gas price -1
    pub fn decr_price(&self) -> Self {
        self.decr_price_by(1)
    }

    /// Returns a new transaction with a lower gas price
    pub fn decr_price_by(&self, value: u128) -> Self {
        self.clone().with_gas_price(self.get_gas_price().checked_sub(value).unwrap())
    }

    /// Returns a new transaction with a higher value
    pub fn inc_value(&self) -> Self {
        self.clone().with_value(self.get_value().checked_add(U256::from(1)).unwrap())
    }

    /// Returns a new transaction with a higher gas limit
    pub fn inc_limit(&self) -> Self {
        self.clone().with_gas_limit(self.get_gas_limit() + 1)
    }

    /// Returns a new transaction with a higher blob fee +1
    ///
    /// If it's an EIP-4844 transaction.
    pub fn inc_blob_fee(&self) -> Self {
        self.inc_blob_fee_by(1)
    }

    /// Returns a new transaction with a higher blob fee
    ///
    /// If it's an EIP-4844 transaction.
    pub fn inc_blob_fee_by(&self, value: u128) -> Self {
        let mut this = self.clone();
        if let MockTransaction::Eip4844 { max_fee_per_blob_gas, .. } = &mut this {
            *max_fee_per_blob_gas = max_fee_per_blob_gas.checked_add(value).unwrap();
        }
        this
    }

    /// Returns a new transaction with a lower blob fee -1
    ///
    /// If it's an EIP-4844 transaction.
    pub fn decr_blob_fee(&self) -> Self {
        self.decr_price_by(1)
    }

    /// Returns a new transaction with a lower blob fee
    ///
    /// If it's an EIP-4844 transaction.
    pub fn decr_blob_fee_by(&self, value: u128) -> Self {
        let mut this = self.clone();
        if let MockTransaction::Eip4844 { max_fee_per_blob_gas, .. } = &mut this {
            *max_fee_per_blob_gas = max_fee_per_blob_gas.checked_sub(value).unwrap();
        }
        this
    }

    /// Returns the transaction type identifier associated with the current [MockTransaction].
    pub const fn tx_type(&self) -> u8 {
        match self {
            Self::Legacy { .. } => LEGACY_TX_TYPE_ID,
            Self::Eip1559 { .. } => EIP1559_TX_TYPE_ID,
            Self::Eip4844 { .. } => EIP4844_TX_TYPE_ID,
            Self::Eip2930 { .. } => EIP2930_TX_TYPE_ID,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => DEPOSIT_TX_TYPE_ID,
        }
    }

    /// Checks if the transaction is of the legacy type.
    pub const fn is_legacy(&self) -> bool {
        matches!(self, MockTransaction::Legacy { .. })
    }

    /// Checks if the transaction is of the EIP-1559 type.
    pub const fn is_eip1559(&self) -> bool {
        matches!(self, MockTransaction::Eip1559 { .. })
    }

    /// Checks if the transaction is of the EIP-4844 type.
    pub const fn is_eip4844(&self) -> bool {
        matches!(self, MockTransaction::Eip4844 { .. })
    }

    /// Checks if the transaction is of the EIP-2930 type.
    pub const fn is_eip2930(&self) -> bool {
        matches!(self, MockTransaction::Eip2930 { .. })
    }
}

impl PoolTransaction for MockTransaction {
    fn hash(&self) -> &TxHash {
        match self {
            MockTransaction::Legacy { hash, .. } |
            MockTransaction::Eip1559 { hash, .. } |
            MockTransaction::Eip4844 { hash, .. } |
            MockTransaction::Eip2930 { hash, .. } => hash,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(TxDeposit { source_hash, .. }) => source_hash,
        }
    }

    fn sender(&self) -> Address {
        match self {
            MockTransaction::Legacy { sender, .. } |
            MockTransaction::Eip1559 { sender, .. } |
            MockTransaction::Eip4844 { sender, .. } |
            MockTransaction::Eip2930 { sender, .. } => *sender,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(TxDeposit { from, .. }) => *from,
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            MockTransaction::Legacy { nonce, .. } |
            MockTransaction::Eip1559 { nonce, .. } |
            MockTransaction::Eip4844 { nonce, .. } |
            MockTransaction::Eip2930 { nonce, .. } => *nonce,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u64,
        }
    }

    fn cost(&self) -> U256 {
        match self {
            MockTransaction::Legacy { gas_price, value, gas_limit, .. } |
            MockTransaction::Eip2930 { gas_limit, gas_price, value, .. } => {
                U256::from(*gas_limit) * U256::from(*gas_price) + *value
            }
            MockTransaction::Eip1559 { max_fee_per_gas, value, gas_limit, .. } |
            MockTransaction::Eip4844 { max_fee_per_gas, value, gas_limit, .. } => {
                U256::from(*gas_limit) * U256::from(*max_fee_per_gas) + *value
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
            MockTransaction::Legacy { gas_price, .. } |
            MockTransaction::Eip2930 { gas_price, .. } => *gas_price,
            MockTransaction::Eip1559 { max_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_fee_per_gas, .. } => *max_fee_per_gas,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u128,
        }
    }

    fn access_list(&self) -> Option<&AccessList> {
        match self {
            MockTransaction::Legacy { .. } => None,
            MockTransaction::Eip1559 { accesslist, .. } |
            MockTransaction::Eip4844 { accesslist, .. } |
            MockTransaction::Eip2930 { accesslist, .. } => Some(accesslist),
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => None,
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            MockTransaction::Legacy { .. } | MockTransaction::Eip2930 { .. } => None,
            MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_priority_fee_per_gas, .. } => {
                Some(*max_priority_fee_per_gas)
            }
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

    /// Calculates the effective tip per gas given a base fee.
    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        // Convert base_fee to u128 for precision in calculations
        let base_fee = base_fee as u128;

        // Retrieve the maximum fee per gas
        let max_fee_per_gas = self.max_fee_per_gas();

        // If the maximum fee per gas is less than the base fee, return None
        if max_fee_per_gas < base_fee {
            return None
        }

        // Calculate the fee by subtracting the base fee from the maximum fee per gas
        let fee = max_fee_per_gas - base_fee;

        // If the maximum priority fee per gas is available, return the minimum of fee and priority
        // fee
        if let Some(priority_fee) = self.max_priority_fee_per_gas() {
            return Some(fee.min(priority_fee))
        }

        // Otherwise, return the calculated fee
        Some(fee)
    }

    /// Returns the priority fee or gas price based on the transaction type.
    fn priority_fee_or_price(&self) -> u128 {
        match self {
            MockTransaction::Legacy { gas_price, .. } |
            MockTransaction::Eip2930 { gas_price, .. } => *gas_price,
            MockTransaction::Eip1559 { max_priority_fee_per_gas, .. } |
            MockTransaction::Eip4844 { max_priority_fee_per_gas, .. } => *max_priority_fee_per_gas,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0u128,
        }
    }

    /// Returns the transaction kind associated with the transaction.
    fn kind(&self) -> &TransactionKind {
        match self {
            MockTransaction::Legacy { to, .. } |
            MockTransaction::Eip1559 { to, .. } |
            MockTransaction::Eip4844 { to, .. } |
            MockTransaction::Eip2930 { to, .. } => to,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(TxDeposit { to, .. }) => to,
        }
    }

    /// Returns the input data associated with the transaction.
    fn input(&self) -> &[u8] {
        match self {
            MockTransaction::Legacy { .. } => &[],
            MockTransaction::Eip1559 { input, .. } |
            MockTransaction::Eip4844 { input, .. } |
            MockTransaction::Eip2930 { input, .. } => input,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit { .. } => &[],
        }
    }

    /// Returns the size of the transaction.
    fn size(&self) -> usize {
        match self {
            MockTransaction::Legacy { size, .. } |
            MockTransaction::Eip1559 { size, .. } |
            MockTransaction::Eip4844 { size, .. } |
            MockTransaction::Eip2930 { size, .. } => *size,
            #[cfg(feature = "optimism")]
            MockTransaction::Deposit(_) => 0,
        }
    }

    /// Returns the transaction type as a byte identifier.
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

    /// Returns the encoded length of the transaction.
    fn encoded_length(&self) -> usize {
        0
    }

    /// Returns the chain ID associated with the transaction.
    fn chain_id(&self) -> Option<u64> {
        Some(1)
    }

    /// Returns true if the transaction is a deposit transaction.
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
        let size = transaction.size();
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
                size,
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
                size,
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
                size,
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
                size,
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
                hash: _,
                sender: _,
                nonce,
                gas_price,
                gas_limit,
                to,
                value,
                input,
                size: _,
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
                hash: _,
                sender: _,
                nonce,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                gas_limit,
                to,
                value,
                accesslist,
                input,
                size: _,
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
                sender: _,
                nonce,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                max_fee_per_blob_gas,
                gas_limit,
                to,
                value,
                accesslist,
                input,
                sidecar: _,
                size: _,
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
                hash: _,
                sender: _,
                nonce,
                to,
                gas_limit,
                input,
                value,
                gas_price,
                accesslist,
                size: _,
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
                    size: tx.size(),
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
                    size: tx.size(),
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
                    size: tx.size(),
                },
                #[allow(unreachable_patterns)]
                _ => unimplemented!(),
            })
            .boxed()
    }

    type Strategy = proptest::strategy::BoxedStrategy<MockTransaction>;
}

/// A factory for creating and managing various types of mock transactions.
#[derive(Debug, Default)]
pub struct MockTransactionFactory {
    pub(crate) ids: SenderIdentifiers,
}

// === impl MockTransactionFactory ===

impl MockTransactionFactory {
    /// Generates a transaction ID for the given [MockTransaction].
    pub fn tx_id(&mut self, tx: &MockTransaction) -> TransactionId {
        let sender = self.ids.sender_id_or_create(tx.get_sender());
        TransactionId::new(sender, tx.get_nonce())
    }

    /// Validates a [MockTransaction] and returns a [MockValidTx].
    pub fn validated(&mut self, transaction: MockTransaction) -> MockValidTx {
        self.validated_with_origin(TransactionOrigin::External, transaction)
    }

    /// Validates a [MockTransaction] and returns a shared [`Arc<MockValidTx>`].
    pub fn validated_arc(&mut self, transaction: MockTransaction) -> Arc<MockValidTx> {
        Arc::new(self.validated(transaction))
    }

    /// Converts the transaction into a validated transaction with a specified origin.
    pub fn validated_with_origin(
        &mut self,
        origin: TransactionOrigin,
        transaction: MockTransaction,
    ) -> MockValidTx {
        MockValidTx {
            propagate: false,
            transaction_id: self.tx_id(&transaction),
            transaction,
            timestamp: Instant::now(),
            origin,
        }
    }

    /// Creates a validated legacy [MockTransaction].
    pub fn create_legacy(&mut self) -> MockValidTx {
        self.validated(MockTransaction::legacy())
    }

    /// Creates a validated EIP-1559 [MockTransaction].
    pub fn create_eip1559(&mut self) -> MockValidTx {
        self.validated(MockTransaction::eip1559())
    }

    /// Creates a validated EIP-4844 [MockTransaction].
    pub fn create_eip4844(&mut self) -> MockValidTx {
        self.validated(MockTransaction::eip4844())
    }
}

/// MockOrdering is just a CoinbaseTipOrdering with MockTransaction
pub type MockOrdering = CoinbaseTipOrdering<MockTransaction>;

/// A ratio of each of the configured transaction types. The percentages sum up to 100, this is
/// enforced in [MockTransactionRatio::new] by an assert.
#[derive(Debug, Clone)]
pub struct MockTransactionRatio {
    /// Percent of transactions that are legacy transactions
    pub legacy_pct: u32,
    /// Percent of transactions that are access list transactions
    pub access_list_pct: u32,
    /// Percent of transactions that are EIP-1559 transactions
    pub dynamic_fee_pct: u32,
    /// Percent of transactions that are EIP-4844 transactions
    pub blob_pct: u32,
}

impl MockTransactionRatio {
    /// Creates a new [MockTransactionRatio] with the given percentages.
    ///
    /// Each argument is treated as a full percent, for example `30u32` is `30%`.
    ///
    /// The percentages must sum up to 100 exactly, or this method will panic.
    pub fn new(legacy_pct: u32, access_list_pct: u32, dynamic_fee_pct: u32, blob_pct: u32) -> Self {
        let total = legacy_pct + access_list_pct + dynamic_fee_pct + blob_pct;
        assert_eq!(
            total,
            100,
            "percentages must sum up to 100, instead got legacy: {legacy_pct}, access_list: {access_list_pct}, dynamic_fee: {dynamic_fee_pct}, blob: {blob_pct}, total: {total}",
        );

        Self { legacy_pct, access_list_pct, dynamic_fee_pct, blob_pct }
    }

    /// Create a [WeightedIndex] from this transaction ratio.
    ///
    /// This index will sample in the following order:
    /// * Legacy transaction => 0
    /// * EIP-2930 transaction => 1
    /// * EIP-1559 transaction => 2
    /// * EIP-4844 transaction => 3
    pub fn weighted_index(&self) -> WeightedIndex<u32> {
        WeightedIndex::new([
            self.legacy_pct,
            self.access_list_pct,
            self.dynamic_fee_pct,
            self.blob_pct,
        ])
        .unwrap()
    }
}

/// The range of each type of fee, for the different transaction types
#[derive(Debug, Clone)]
pub struct MockFeeRange {
    /// The range of gas_price or legacy and access list transactions
    pub gas_price: Uniform<u128>,
    /// The range of priority fees for EIP-1559 and EIP-4844 transactions
    pub priority_fee: Uniform<u128>,
    /// The range of max fees for EIP-1559 and EIP-4844 transactions
    pub max_fee: Uniform<u128>,
    /// The range of max fees per blob gas for EIP-4844 transactions
    pub max_fee_blob: Uniform<u128>,
}

impl MockFeeRange {
    /// Creates a new [MockFeeRange] with the given ranges.
    ///
    /// Expects the bottom of the `priority_fee_range` to be greater than the top of the
    /// `max_fee_range`.
    pub fn new(
        gas_price: Range<u128>,
        priority_fee: Range<u128>,
        max_fee: Range<u128>,
        max_fee_blob: Range<u128>,
    ) -> Self {
        assert!(
            max_fee.start <= priority_fee.end,
            "max_fee_range should be strictly below the priority fee range"
        );
        Self {
            gas_price: gas_price.into(),
            priority_fee: priority_fee.into(),
            max_fee: max_fee.into(),
            max_fee_blob: max_fee_blob.into(),
        }
    }

    /// Returns a sample of `gas_price` for legacy and access list transactions with the given
    /// [Rng](rand::Rng).
    pub fn sample_gas_price(&self, rng: &mut impl rand::Rng) -> u128 {
        self.gas_price.sample(rng)
    }

    /// Returns a sample of `max_priority_fee_per_gas` for EIP-1559 and EIP-4844 transactions with
    /// the given [Rng](rand::Rng).
    pub fn sample_priority_fee(&self, rng: &mut impl rand::Rng) -> u128 {
        self.priority_fee.sample(rng)
    }

    /// Returns a sample of `max_fee_per_gas` for EIP-1559 and EIP-4844 transactions with the given
    /// [Rng](rand::Rng).
    pub fn sample_max_fee(&self, rng: &mut impl rand::Rng) -> u128 {
        self.max_fee.sample(rng)
    }

    /// Returns a sample of `max_fee_per_blob_gas` for EIP-4844 transactions with the given
    /// [Rng](rand::Rng).
    pub fn sample_max_fee_blob(&self, rng: &mut impl rand::Rng) -> u128 {
        self.max_fee_blob.sample(rng)
    }
}

/// A configured distribution that can generate transactions
#[derive(Debug, Clone)]
pub struct MockTransactionDistribution {
    /// ratio of each transaction type to generate
    transaction_ratio: MockTransactionRatio,
    /// generates the gas limit
    gas_limit_range: Uniform<u64>,
    /// generates the transaction's fake size
    size_range: Uniform<usize>,
    /// generates fees for the given transaction types
    fee_ranges: MockFeeRange,
}

impl MockTransactionDistribution {
    /// Creates a new generator distribution.
    pub fn new(
        transaction_ratio: MockTransactionRatio,
        fee_ranges: MockFeeRange,
        gas_limit_range: Range<u64>,
        size_range: Range<usize>,
    ) -> Self {
        Self {
            transaction_ratio,
            gas_limit_range: gas_limit_range.into(),
            fee_ranges,
            size_range: size_range.into(),
        }
    }

    /// Generates a new transaction
    pub fn tx(&self, nonce: u64, rng: &mut impl rand::Rng) -> MockTransaction {
        let transaction_sample = self.transaction_ratio.weighted_index().sample(rng);
        let tx = match transaction_sample {
            0 => MockTransaction::legacy().with_gas_price(self.fee_ranges.sample_gas_price(rng)),
            1 => MockTransaction::eip2930().with_gas_price(self.fee_ranges.sample_gas_price(rng)),
            2 => MockTransaction::eip1559()
                .with_priority_fee(self.fee_ranges.sample_priority_fee(rng))
                .with_max_fee(self.fee_ranges.sample_max_fee(rng)),
            3 => MockTransaction::eip4844()
                .with_priority_fee(self.fee_ranges.sample_priority_fee(rng))
                .with_max_fee(self.fee_ranges.sample_max_fee(rng))
                .with_blob_fee(self.fee_ranges.sample_max_fee_blob(rng)),
            _ => unreachable!("unknown transaction type returned by the weighted index"),
        };

        let size = self.size_range.sample(rng);

        tx.with_nonce(nonce).with_gas_limit(self.gas_limit_range.sample(rng)).with_size(size)
    }

    /// Generates a new transaction set for the given sender.
    ///
    /// The nonce range defines which nonces to set, and how many transactions to generate.
    pub fn tx_set(
        &self,
        sender: Address,
        nonce_range: Range<u64>,
        rng: &mut impl rand::Rng,
    ) -> MockTransactionSet {
        let mut txs = Vec::new();
        for nonce in nonce_range {
            txs.push(self.tx(nonce, rng).with_sender(sender));
        }
        MockTransactionSet::new(txs)
    }

    /// Generates a transaction set that ensures that blob txs are not mixed with other transaction
    /// types.
    ///
    /// This is done by taking the existing distribution, and using the first transaction to
    /// determine whether or not the sender should generate entirely blob transactions.
    pub fn tx_set_non_conflicting_types(
        &self,
        sender: Address,
        nonce_range: Range<u64>,
        rng: &mut impl rand::Rng,
    ) -> NonConflictingSetOutcome {
        // This will create a modified distribution that will only generate blob transactions
        // for the given sender, if the blob transaction is the first transaction in the set.
        //
        // Otherwise, it will modify the transaction distribution to only generate legacy, eip2930,
        // and eip1559 transactions.
        //
        // The new distribution should still have the same relative amount of transaction types.
        let mut modified_distribution = self.clone();
        let first_tx = self.tx(nonce_range.start, rng);

        // now we can check and modify the distribution, preserving potentially uneven ratios
        // between transaction types
        if first_tx.is_eip4844() {
            modified_distribution.transaction_ratio = MockTransactionRatio {
                legacy_pct: 0,
                access_list_pct: 0,
                dynamic_fee_pct: 0,
                blob_pct: 100,
            };

            // finally generate the transaction set
            NonConflictingSetOutcome::BlobsOnly(modified_distribution.tx_set(
                sender,
                nonce_range,
                rng,
            ))
        } else {
            let MockTransactionRatio { legacy_pct, access_list_pct, dynamic_fee_pct, .. } =
                modified_distribution.transaction_ratio;

            // Calculate the total weight of non-blob transactions
            let total_non_blob_weight: u32 = legacy_pct + access_list_pct + dynamic_fee_pct;

            // Calculate new weights, preserving the ratio between non-blob transaction types
            let new_weights: Vec<u32> = [legacy_pct, access_list_pct, dynamic_fee_pct]
                .into_iter()
                .map(|weight| weight * 100 / total_non_blob_weight)
                .collect();

            let new_ratio = MockTransactionRatio {
                legacy_pct: new_weights[0],
                access_list_pct: new_weights[1],
                dynamic_fee_pct: new_weights[2],
                blob_pct: 0,
            };

            // Set the new transaction ratio excluding blob transactions and preserving the relative
            // ratios
            modified_distribution.transaction_ratio = new_ratio;

            // finally generate the transaction set
            NonConflictingSetOutcome::Mixed(modified_distribution.tx_set(sender, nonce_range, rng))
        }
    }
}

/// Indicates whether or not the non-conflicting transaction set generated includes only blobs, or
/// a mix of transaction types.
#[derive(Debug, Clone)]
pub enum NonConflictingSetOutcome {
    /// The transaction set includes only blob transactions
    BlobsOnly(MockTransactionSet),
    /// The transaction set includes a mix of transaction types
    Mixed(MockTransactionSet),
}

impl NonConflictingSetOutcome {
    /// Returns the inner [MockTransactionSet]
    pub fn into_inner(self) -> MockTransactionSet {
        match self {
            NonConflictingSetOutcome::BlobsOnly(set) | NonConflictingSetOutcome::Mixed(set) => set,
        }
    }

    /// Introduces artificial nonce gaps into the transaction set, at random, with a range of gap
    /// sizes.
    ///
    /// If this is a [NonConflictingSetOutcome::BlobsOnly], then nonce gaps will not be introduced.
    /// Otherwise, the nonce gaps will be introduced to the mixed transaction set.
    ///
    /// See [MockTransactionSet::with_nonce_gaps] for more information on the generation process.
    pub fn with_nonce_gaps(
        &mut self,
        gap_pct: u32,
        gap_range: Range<u64>,
        rng: &mut impl rand::Rng,
    ) {
        match self {
            NonConflictingSetOutcome::BlobsOnly(_) => {}
            NonConflictingSetOutcome::Mixed(set) => set.with_nonce_gaps(gap_pct, gap_range, rng),
        }
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

    /// Creates a series of dependent transactions for a given sender and nonce.
    ///
    /// This method generates a sequence of transactions starting from the provided nonce
    /// for the given sender.
    ///
    /// The number of transactions created is determined by `tx_count`.
    pub fn dependent(sender: Address, from_nonce: u64, tx_count: usize, tx_type: TxType) -> Self {
        let mut txs = Vec::with_capacity(tx_count);
        let mut curr_tx =
            MockTransaction::new_from_type(tx_type).with_nonce(from_nonce).with_sender(sender);
        for _ in 0..tx_count {
            txs.push(curr_tx.clone());
            curr_tx = curr_tx.next();
        }

        Self::new(txs)
    }

    /// Creates a chain of transactions for a given sender with a specified count.
    ///
    /// This method generates a sequence of transactions starting from the specified sender
    /// and creates a chain of transactions based on the `tx_count`.
    pub fn sequential_transactions_by_sender(
        sender: Address,
        tx_count: usize,
        tx_type: TxType,
    ) -> Self {
        Self::dependent(sender, 0, tx_count, tx_type)
    }

    /// Introduces artificial nonce gaps into the transaction set, at random, with a range of gap
    /// sizes.
    ///
    /// This assumes that the `gap_pct` is between 0 and 100, and the `gap_range` has a lower bound
    /// of at least one. This is enforced with assertions.
    ///
    /// The `gap_pct` is the percent chance that the next transaction in the set will introduce a
    /// nonce gap.
    ///
    /// Let an example transaction set be `[(tx1, 1), (tx2, 2)]`, where the first element of the
    /// tuple is a transaction, and the second element is the nonce. If the `gap_pct` is 50, and
    /// the `gap_range` is `1..=1`, then the resulting transaction set could would be either
    /// `[(tx1, 1), (tx2, 2)]` or `[(tx1, 1), (tx2, 3)]`, with a 50% chance of either.
    pub fn with_nonce_gaps(
        &mut self,
        gap_pct: u32,
        gap_range: Range<u64>,
        rng: &mut impl rand::Rng,
    ) {
        assert!(gap_pct <= 100, "gap_pct must be between 0 and 100");
        assert!(gap_range.start >= 1, "gap_range must have a lower bound of at least one");

        let mut prev_nonce = 0;
        for tx in self.transactions.iter_mut() {
            if rng.gen_bool(gap_pct as f64 / 100.0) {
                prev_nonce += gap_range.start;
            } else {
                prev_nonce += 1;
            }
            tx.set_nonce(prev_nonce);
        }
    }

    /// Add transactions to the [MockTransactionSet]
    pub fn extend<T: IntoIterator<Item = MockTransaction>>(&mut self, txs: T) {
        self.transactions.extend(txs);
    }

    /// Extract the inner [Vec] of [MockTransaction]s
    pub fn into_vec(self) -> Vec<MockTransaction> {
        self.transactions
    }

    /// Returns an iterator over the contained transactions in the set
    pub fn iter(&self) -> impl Iterator<Item = &MockTransaction> {
        self.transactions.iter()
    }

    /// Returns a mutable iterator over the contained transactions in the set.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut MockTransaction> {
        self.transactions.iter_mut()
    }
}

impl IntoIterator for MockTransactionSet {
    type Item = MockTransaction;
    type IntoIter = IntoIter<MockTransaction>;

    fn into_iter(self) -> Self::IntoIter {
        self.transactions.into_iter()
    }
}

#[test]
fn test_mock_priority() {
    use crate::TransactionOrdering;

    let o = MockOrdering::default();
    let lo = MockTransaction::eip1559().with_gas_limit(100_000);
    let hi = lo.next().inc_price();
    assert!(o.priority(&hi, 0) > o.priority(&lo, 0));
}
