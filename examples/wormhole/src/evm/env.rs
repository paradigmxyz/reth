use crate::primitives::{WormholeTransaction, WormholeTransactionSigned};
use alloy_eips::eip2930::AccessList;
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded, IntoTxEnv};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use op_alloy_consensus::OpTxEnvelope;
use op_revm::OpTransaction;
use reth_op::evm::primitives::TransactionEnv;
use revm::context::TxEnv;

/// A Wormhole transaction environment that extends Optimism transactions
#[derive(Clone, Debug)]
pub enum WormholeTxEnv {
    Op(OpTransaction<TxEnv>),
    Wormhole(WormholeOpTxEnv),
}

/// An Optimism transaction environment for Wormhole transactions
#[derive(Clone, Debug, Default)]
pub struct WormholeOpTxEnv(pub TxEnv);

impl revm::context::Transaction for WormholeTxEnv {
    type AccessListItem<'a>
        = <TxEnv as revm::context::Transaction>::AccessListItem<'a>
    where
        Self: 'a;
    type Authorization<'a>
        = <TxEnv as revm::context::Transaction>::Authorization<'a>
    where
        Self: 'a;

    fn tx_type(&self) -> u8 {
        match self {
            Self::Op(tx) => tx.tx_type(),
            Self::Wormhole(tx) => tx.tx_type(),
        }
    }

    fn caller(&self) -> Address {
        match self {
            Self::Op(tx) => tx.caller(),
            Self::Wormhole(tx) => tx.caller(),
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Op(tx) => tx.gas_limit(),
            Self::Wormhole(tx) => tx.gas_limit(),
        }
    }

    fn value(&self) -> U256 {
        match self {
            Self::Op(tx) => tx.value(),
            Self::Wormhole(tx) => tx.value(),
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Op(tx) => tx.input(),
            Self::Wormhole(tx) => tx.input(),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Op(tx) => revm::context::Transaction::nonce(tx),
            Self::Wormhole(tx) => revm::context::Transaction::nonce(tx),
        }
    }

    fn kind(&self) -> TxKind {
        match self {
            Self::Op(tx) => tx.kind(),
            Self::Wormhole(tx) => tx.kind(),
        }
    }

    fn chain_id(&self) -> Option<u64> {
        match self {
            Self::Op(tx) => tx.chain_id(),
            Self::Wormhole(tx) => tx.chain_id(),
        }
    }

    fn gas_price(&self) -> u128 {
        match self {
            Self::Op(tx) => tx.gas_price(),
            Self::Wormhole(tx) => tx.gas_price(),
        }
    }

    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        Some(match self {
            Self::Op(tx) => tx.base.access_list.iter(),
            Self::Wormhole(tx) => tx.0.access_list.iter(),
        })
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        match self {
            Self::Op(tx) => tx.blob_versioned_hashes(),
            Self::Wormhole(tx) => tx.blob_versioned_hashes(),
        }
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        match self {
            Self::Op(tx) => tx.max_fee_per_blob_gas(),
            Self::Wormhole(tx) => tx.max_fee_per_blob_gas(),
        }
    }

    fn authorization_list_len(&self) -> usize {
        match self {
            Self::Op(tx) => tx.authorization_list_len(),
            Self::Wormhole(tx) => tx.authorization_list_len(),
        }
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        match self {
            Self::Op(tx) => tx.base.authorization_list.iter(),
            Self::Wormhole(tx) => tx.0.authorization_list.iter(),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Op(tx) => tx.max_priority_fee_per_gas(),
            Self::Wormhole(tx) => tx.max_priority_fee_per_gas(),
        }
    }
}

impl revm::context::Transaction for WormholeOpTxEnv {
    type AccessListItem<'a>
        = <TxEnv as revm::context::Transaction>::AccessListItem<'a>
    where
        Self: 'a;
    type Authorization<'a>
        = <TxEnv as revm::context::Transaction>::Authorization<'a>
    where
        Self: 'a;

    fn tx_type(&self) -> u8 {
        self.0.tx_type()
    }

    fn caller(&self) -> Address {
        self.0.caller()
    }

    fn gas_limit(&self) -> u64 {
        self.0.gas_limit()
    }

    fn value(&self) -> U256 {
        self.0.value()
    }

    fn input(&self) -> &Bytes {
        self.0.input()
    }

    fn nonce(&self) -> u64 {
        revm::context::Transaction::nonce(&self.0)
    }

    fn kind(&self) -> TxKind {
        self.0.kind()
    }

    fn chain_id(&self) -> Option<u64> {
        self.0.chain_id()
    }

    fn gas_price(&self) -> u128 {
        self.0.gas_price()
    }

    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        self.0.access_list()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.0.blob_versioned_hashes()
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        self.0.max_fee_per_blob_gas()
    }

    fn authorization_list_len(&self) -> usize {
        self.0.authorization_list_len()
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        self.0.authorization_list()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.0.max_priority_fee_per_gas()
    }
}

impl TransactionEnv for WormholeOpTxEnv {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.0.set_gas_limit(gas_limit);
    }

    fn nonce(&self) -> u64 {
        self.0.nonce()
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.0.set_nonce(nonce);
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.0.set_access_list(access_list);
    }
}

impl TransactionEnv for WormholeTxEnv {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        match self {
            Self::Op(tx) => tx.set_gas_limit(gas_limit),
            Self::Wormhole(tx) => tx.set_gas_limit(gas_limit),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Op(tx) => tx.nonce(),
            Self::Wormhole(tx) => tx.nonce(),
        }
    }

    fn set_nonce(&mut self, nonce: u64) {
        match self {
            Self::Op(tx) => tx.set_nonce(nonce),
            Self::Wormhole(tx) => tx.set_nonce(nonce),
        }
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        match self {
            Self::Op(tx) => tx.set_access_list(access_list),
            Self::Wormhole(tx) => tx.set_access_list(access_list),
        }
    }
}

impl FromRecoveredTx<WormholeTransactionSigned> for WormholeOpTxEnv {
    fn from_recovered_tx(tx: &WormholeTransactionSigned, sender: Address) -> Self {
        WormholeOpTxEnv(match tx.transaction() {
            WormholeTransaction::Optimism(tx) => {
                OpTransaction::<TxEnv>::from_recovered_tx(tx, sender).base
            }
            WormholeTransaction::Wormhole(tx) => {
                // Since we're using OpTransactionSigned for demo, treat as OP transaction
                OpTransaction::<TxEnv>::from_recovered_tx(tx, sender).base
            }
        })
    }
}

impl FromTxWithEncoded<WormholeTransactionSigned> for WormholeOpTxEnv {
    fn from_encoded_tx(tx: &WormholeTransactionSigned, sender: Address, encoded: Bytes) -> Self {
        WormholeOpTxEnv(match tx.transaction() {
            WormholeTransaction::Optimism(tx) => {
                OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded).base
            }
            WormholeTransaction::Wormhole(tx) => {
                // Since we're using OpTransactionSigned for demo, treat as OP transaction
                OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded).base
            }
        })
    }
}

impl FromRecoveredTx<OpTxEnvelope> for WormholeTxEnv {
    fn from_recovered_tx(tx: &OpTxEnvelope, sender: Address) -> Self {
        Self::Op(OpTransaction::from_recovered_tx(tx, sender))
    }
}

impl FromTxWithEncoded<OpTxEnvelope> for WormholeTxEnv {
    fn from_encoded_tx(tx: &OpTxEnvelope, sender: Address, encoded: Bytes) -> Self {
        Self::Op(OpTransaction::from_encoded_tx(tx, sender, encoded))
    }
}

impl FromRecoveredTx<WormholeTransactionSigned> for WormholeTxEnv {
    fn from_recovered_tx(tx: &WormholeTransactionSigned, sender: Address) -> Self {
        match tx.transaction() {
            WormholeTransaction::Optimism(tx) => Self::from_recovered_tx(tx, sender),
            WormholeTransaction::Wormhole(tx) => {
                // For demo, treat wormhole transactions as special OP transactions
                Self::Wormhole(WormholeOpTxEnv(
                    OpTransaction::<TxEnv>::from_recovered_tx(tx, sender).base,
                ))
            }
        }
    }
}

impl FromTxWithEncoded<WormholeTransactionSigned> for WormholeTxEnv {
    fn from_encoded_tx(tx: &WormholeTransactionSigned, sender: Address, encoded: Bytes) -> Self {
        match tx.transaction() {
            WormholeTransaction::Optimism(tx) => Self::from_encoded_tx(tx, sender, encoded),
            WormholeTransaction::Wormhole(tx) => {
                // For demo, treat wormhole transactions as special OP transactions
                Self::Wormhole(WormholeOpTxEnv(
                    OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded).base,
                ))
            }
        }
    }
}

impl IntoTxEnv<Self> for WormholeTxEnv {
    fn into_tx_env(self) -> Self {
        self
    }
}

impl FromRecoveredTx<WormholeTransaction> for WormholeTxEnv {
    fn from_recovered_tx(tx: &WormholeTransaction, sender: Address) -> Self {
        match tx {
            WormholeTransaction::Optimism(tx) => {
                Self::Op(OpTransaction::from_recovered_tx(tx, sender))
            }
            WormholeTransaction::Wormhole(tx) => {
                // For demo, treat wormhole transactions as special OP transactions
                Self::Wormhole(WormholeOpTxEnv(
                    OpTransaction::<TxEnv>::from_recovered_tx(tx, sender).base,
                ))
            }
        }
    }
}

impl FromTxWithEncoded<WormholeTransaction> for WormholeTxEnv {
    fn from_encoded_tx(tx: &WormholeTransaction, sender: Address, encoded: Bytes) -> Self {
        match tx {
            WormholeTransaction::Optimism(tx) => {
                Self::Op(OpTransaction::from_encoded_tx(tx, sender, encoded))
            }
            WormholeTransaction::Wormhole(tx) => {
                // For demo, treat wormhole transactions as special OP transactions
                Self::Wormhole(WormholeOpTxEnv(
                    OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded).base,
                ))
            }
        }
    }
}
