use auto_impl::auto_impl;
use revm::{
    context::TxEnv,
    context_interface::transaction::Transaction,
    primitives::{Address, Bytes, TxKind, B256, U256},
};

#[auto_impl(&, &mut, Box, Arc)]
pub trait BscTxTr: Transaction {
    /// Whether the transaction is a system transaction
    fn is_system_transaction(&self) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BscTransaction<T: Transaction> {
    pub base: T,
    pub is_system_transaction: Option<bool>,
}

impl<T: Transaction> BscTransaction<T> {
    pub fn new(base: T) -> Self {
        Self { base, is_system_transaction: None }
    }
}

impl Default for BscTransaction<TxEnv> {
    fn default() -> Self {
        Self { base: TxEnv::default(), is_system_transaction: None }
    }
}

impl<T: Transaction> Transaction for BscTransaction<T> {
    type AccessList = T::AccessList;
    type Authorization = T::Authorization;

    fn tx_type(&self) -> u8 {
        self.base.tx_type()
    }

    fn caller(&self) -> Address {
        self.base.caller()
    }

    fn gas_limit(&self) -> u64 {
        self.base.gas_limit()
    }

    fn value(&self) -> U256 {
        self.base.value()
    }

    fn input(&self) -> &Bytes {
        self.base.input()
    }

    fn nonce(&self) -> u64 {
        self.base.nonce()
    }

    fn kind(&self) -> TxKind {
        self.base.kind()
    }

    fn chain_id(&self) -> Option<u64> {
        self.base.chain_id()
    }

    fn access_list(&self) -> Option<&Self::AccessList> {
        self.base.access_list()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.base.max_priority_fee_per_gas()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.base.max_fee_per_gas()
    }

    fn gas_price(&self) -> u128 {
        self.base.gas_price()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.base.blob_versioned_hashes()
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        self.base.max_fee_per_blob_gas()
    }

    fn effective_gas_price(&self, base_fee: u128) -> u128 {
        self.base.effective_gas_price(base_fee)
    }

    fn authorization_list_len(&self) -> usize {
        self.base.authorization_list_len()
    }

    fn authorization_list(&self) -> impl Iterator<Item = &Self::Authorization> {
        self.base.authorization_list()
    }
}

impl<T: Transaction> BscTxTr for BscTransaction<T> {
    fn is_system_transaction(&self) -> bool {
        self.is_system_transaction.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use revm::primitives::Address;

    #[test]
    fn test_bsc_transaction_fields() {
        let bsc_tx = BscTransaction {
            base: TxEnv {
                tx_type: 0,
                gas_limit: 10,
                gas_price: 100,
                gas_priority_fee: Some(5),
                ..Default::default()
            },
            is_system_transaction: None,
        };

        assert_eq!(bsc_tx.tx_type(), 0);
        assert_eq!(bsc_tx.gas_limit(), 10);
        assert_eq!(bsc_tx.kind(), revm::primitives::TxKind::Call(Address::ZERO));
        assert_eq!(bsc_tx.effective_gas_price(90), 95);
        assert_eq!(bsc_tx.max_fee_per_gas(), 100);
    }
}
