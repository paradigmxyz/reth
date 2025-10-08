use alloy_consensus::transaction::{Recovered, TxHashRef};
use alloy_eips::Encodable2718;
use alloy_consensus::{BlobTransactionValidationError, Typed2718};
use alloy_eips::eip2718::WithEncoded;
use alloy_eips::eip2930::AccessList;
use alloy_eips::eip7594::BlobTransactionSidecarVariant;
use alloy_primitives::{Address, Bytes, TxHash, TxKind, U256, B256};
use c_kzg::KzgSettings;
use core::convert::Infallible;
use core::fmt::Debug;
use reth_arbitrum_primitives::ArbTransactionSigned;
use reth_primitives_traits::{InMemorySize, SignedTransaction};
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction, PoolTransaction,
};

#[derive(Debug, Clone, derive_more::Deref)]
pub struct ArbPooledTransaction {
    #[deref]
    inner: EthPooledTransaction<ArbTransactionSigned>,
}

impl ArbPooledTransaction {
    pub fn new(transaction: Recovered<ArbTransactionSigned>, encoded_length: usize) -> Self {
        Self { inner: EthPooledTransaction::new(transaction, encoded_length) }
    }
}

impl PoolTransaction for ArbPooledTransaction {
    type TryFromConsensusError = Infallible;
    type Consensus = ArbTransactionSigned;
    type Pooled = ArbTransactionSigned;

    fn clone_into_consensus(&self) -> Recovered<Self::Consensus> {
        self.inner.transaction().clone()
    }

    fn into_consensus(self) -> Recovered<Self::Consensus> {
        self.inner.transaction
    }

    fn into_consensus_with2718(self) -> WithEncoded<Recovered<Self::Consensus>> {
        let encoding: Bytes = self.inner.transaction().encoded_2718().into();
        self.inner.transaction.into_encoded_with(encoding)
    }

    fn from_pooled(tx: Recovered<Self::Pooled>) -> Self {
        let encoded_len = tx.encode_2718_len();
        Self::new(tx, encoded_len)
    }

    fn hash(&self) -> &TxHash {
        self.inner.transaction.tx_hash()
    }

    fn sender(&self) -> Address {
        self.inner.transaction.signer()
    }

    fn sender_ref(&self) -> &Address {
        self.inner.transaction.signer_ref()
    }

    fn cost(&self) -> &U256 {
        &self.inner.cost
    }

    fn encoded_length(&self) -> usize {
        self.inner.encoded_length
    }
}

impl InMemorySize for ArbPooledTransaction {
    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl alloy_consensus::Transaction for ArbPooledTransaction {
    fn chain_id(&self) -> Option<u64> {
        self.inner.chain_id()
    }
    fn nonce(&self) -> u64 {
        self.inner.nonce()
    }
    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit()
    }
    fn gas_price(&self) -> Option<u128> {
        self.inner.gas_price()
    }
    fn max_fee_per_gas(&self) -> u128 {
        self.inner.max_fee_per_gas()
    }
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.inner.max_priority_fee_per_gas()
    }
    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.inner.max_fee_per_blob_gas()
    }
    fn priority_fee_or_price(&self) -> u128 {
        self.inner.priority_fee_or_price()
    }
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.inner.effective_gas_price(base_fee)
    }
    fn is_dynamic_fee(&self) -> bool {
        self.inner.is_dynamic_fee()
    }
    fn kind(&self) -> TxKind {
        self.inner.kind()
    }
    fn is_create(&self) -> bool {
        self.inner.is_create()
    }
    fn value(&self) -> U256 {
        self.inner.value()
    }
    fn input(&self) -> &Bytes {
        self.inner.input()
    }
    fn access_list(&self) -> Option<&AccessList> {
        self.inner.access_list()
    }
    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.inner.blob_versioned_hashes()
    }
    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        None
    }
}

impl Typed2718 for ArbPooledTransaction {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl EthPoolTransaction for ArbPooledTransaction {
    fn take_blob(&mut self) -> EthBlobTransactionSidecar {
        EthBlobTransactionSidecar::None
    }

    fn try_into_pooled_eip4844(
        self,
        _sidecar: std::sync::Arc<BlobTransactionSidecarVariant>,
    ) -> Option<Recovered<Self::Pooled>> {
        None
    }

    fn try_from_eip4844(
        _tx: Recovered<Self::Consensus>,
        _sidecar: BlobTransactionSidecarVariant,
    ) -> Option<Self> {
        None
    }

    fn validate_blob(
        &self,
        _blob: &BlobTransactionSidecarVariant,
        _settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        Err(BlobTransactionValidationError::NotBlobTransaction(self.ty()))
    }
}
