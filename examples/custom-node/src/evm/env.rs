use crate::primitives::{CustomTransaction, CustomTransactionEnvelope, TxPayment};
use alloy_eips::{
    eip2718::{EIP2930_TX_TYPE_ID, LEGACY_TX_TYPE_ID},
    eip2930::AccessList,
    Typed2718,
};
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded, IntoTxEnv};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use op_revm::OpTransaction;
use reth_ethereum::evm::{primitives::TransactionEnv, revm::context::TxEnv};

/// An Optimism extended Ethereum transaction that can be fed to [`Evm`] because it contains
/// [`CustomTxEnv`].
///
/// [`Evm`]: alloy_evm::Evm
#[derive(Clone, Debug)]
pub struct CustomEvmTransaction(pub OpTransaction<CustomTxEnv>);

/// A transaction environment is a set of information related to an Ethereum transaction that can be
/// fed to [`Evm`] for execution.
///
/// [`Evm`]: alloy_evm::Evm
#[derive(Clone, Debug, Default)]
pub struct CustomTxEnv(pub TxEnv);

impl revm::context::Transaction for CustomEvmTransaction {
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

impl revm::context::Transaction for CustomTxEnv {
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

impl TransactionEnv for CustomTxEnv {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.0.gas_limit = gas_limit;
    }

    fn nonce(&self) -> u64 {
        self.0.nonce
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.0.nonce = nonce;
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.0.access_list = access_list;

        if self.0.tx_type == LEGACY_TX_TYPE_ID {
            // if this was previously marked as legacy tx, this must be upgraded to eip2930 with an
            // accesslist
            self.0.tx_type = EIP2930_TX_TYPE_ID;
        }
    }
}

impl TransactionEnv for CustomEvmTransaction {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.0.base.set_gas_limit(gas_limit)
    }

    fn nonce(&self) -> u64 {
        self.0.base.nonce()
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.0.base.set_nonce(nonce)
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.0.base.set_access_list(access_list)
    }
}

impl FromRecoveredTx<CustomTransaction> for CustomTxEnv {
    fn from_recovered_tx(tx: &CustomTransaction, sender: Address) -> Self {
        CustomTxEnv(match tx {
            CustomTransaction::BuiltIn(tx) => {
                OpTransaction::<TxEnv>::from_recovered_tx(tx, sender).base
            }
            CustomTransaction::Other(tx) => TxEnv::from_recovered_tx(tx, sender),
        })
    }
}

impl FromTxWithEncoded<CustomTransaction> for CustomTxEnv {
    fn from_encoded_tx(tx: &CustomTransaction, sender: Address, encoded: Bytes) -> Self {
        CustomTxEnv(match tx {
            CustomTransaction::BuiltIn(tx) => {
                OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded).base
            }
            CustomTransaction::Other(tx) => TxEnv::from_encoded_tx(tx, sender, encoded),
        })
    }
}

impl FromRecoveredTx<TxPayment> for TxEnv {
    fn from_recovered_tx(tx: &TxPayment, caller: Address) -> Self {
        let TxPayment {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
        } = tx;
        Self {
            tx_type: tx.ty(),
            caller,
            gas_limit: *gas_limit,
            gas_price: *max_fee_per_gas,
            gas_priority_fee: Some(*max_priority_fee_per_gas),
            kind: TxKind::Call(*to),
            value: *value,
            nonce: *nonce,
            chain_id: Some(*chain_id),
            ..Default::default()
        }
    }
}

impl FromRecoveredTx<CustomTransactionEnvelope> for TxEnv {
    fn from_recovered_tx(tx: &CustomTransactionEnvelope, sender: Address) -> Self {
        Self::from_recovered_tx(tx.inner.tx(), sender)
    }
}

impl FromTxWithEncoded<CustomTransactionEnvelope> for TxEnv {
    fn from_encoded_tx(tx: &CustomTransactionEnvelope, sender: Address, _encoded: Bytes) -> Self {
        Self::from_recovered_tx(tx.inner.tx(), sender)
    }
}

impl FromRecoveredTx<CustomTransaction> for CustomEvmTransaction {
    fn from_recovered_tx(tx: &CustomTransaction, sender: Address) -> Self {
        Self(match tx {
            CustomTransaction::BuiltIn(tx) => {
                let tx = OpTransaction::<TxEnv>::from_recovered_tx(tx, sender);
                let base = CustomTxEnv(tx.base);

                OpTransaction { base, enveloped_tx: tx.enveloped_tx, deposit: tx.deposit }
            }
            CustomTransaction::Other(tx) => {
                OpTransaction::new(CustomTxEnv(TxEnv::from_recovered_tx(tx, sender)))
            }
        })
    }
}

impl FromTxWithEncoded<CustomTransaction> for CustomEvmTransaction {
    fn from_encoded_tx(tx: &CustomTransaction, sender: Address, encoded: Bytes) -> Self {
        Self(match tx {
            CustomTransaction::BuiltIn(tx) => {
                let tx = OpTransaction::<TxEnv>::from_encoded_tx(tx, sender, encoded);
                let base = CustomTxEnv(tx.base);

                OpTransaction { base, enveloped_tx: tx.enveloped_tx, deposit: tx.deposit }
            }
            CustomTransaction::Other(tx) => {
                OpTransaction::new(CustomTxEnv(TxEnv::from_encoded_tx(tx, sender, encoded)))
            }
        })
    }
}

impl IntoTxEnv<Self> for CustomEvmTransaction {
    fn into_tx_env(self) -> Self {
        self
    }
}
