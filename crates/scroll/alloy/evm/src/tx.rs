use alloy_evm::IntoTxEnv;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use core::ops::{Deref, DerefMut};
use revm::context::Transaction;
use revm_scroll::ScrollTransaction;

/// This structure wraps around a [`ScrollTransaction`] and allows us to implement the [`IntoTxEnv`]
/// trait. This can be removed when the interface is improved. Without this wrapper, we would need
/// to implement the trait in `revm-scroll`, which adds a dependency on `alloy-evm` in the crate.
/// Any changes to `alloy-evm` would require changes to `revm-scroll` which isn't desired.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollTransactionIntoTxEnv<T: Transaction>(ScrollTransaction<T>);

impl<T: Transaction> ScrollTransactionIntoTxEnv<T> {
    /// Returns a new [`ScrollTransactionIntoTxEnv`].
    pub fn new(base: T, rlp_bytes: Option<Bytes>) -> Self {
        Self(ScrollTransaction::new(base, rlp_bytes))
    }
}

impl<T: Transaction> From<ScrollTransaction<T>> for ScrollTransactionIntoTxEnv<T> {
    fn from(value: ScrollTransaction<T>) -> Self {
        Self(value)
    }
}

impl<T: Transaction> From<ScrollTransactionIntoTxEnv<T>> for ScrollTransaction<T> {
    fn from(value: ScrollTransactionIntoTxEnv<T>) -> Self {
        value.0
    }
}

impl<T: Transaction> Deref for ScrollTransactionIntoTxEnv<T> {
    type Target = ScrollTransaction<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Transaction> DerefMut for ScrollTransactionIntoTxEnv<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Transaction> IntoTxEnv<Self> for ScrollTransactionIntoTxEnv<T> {
    fn into_tx_env(self) -> Self {
        self
    }
}

impl<T: Transaction> Transaction for ScrollTransactionIntoTxEnv<T> {
    type AccessListItem<'a>
        = T::AccessListItem<'a>
    where
        T: 'a;
    type Authorization<'a>
        = T::Authorization<'a>
    where
        T: 'a;

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
        self.0.nonce()
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

    fn access_list(
        &self,
    ) -> Option<impl Iterator<Item = <ScrollTransaction<T> as Transaction>::AccessListItem<'_>>>
    {
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
