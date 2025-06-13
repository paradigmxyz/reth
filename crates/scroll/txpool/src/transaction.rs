use alloy_consensus::{transaction::Recovered, BlobTransactionValidationError, Typed2718};
use alloy_eips::{
    eip2930::AccessList, eip7594::BlobTransactionSidecarVariant, eip7702::SignedAuthorization,
};
use alloy_primitives::{Address, Bytes, TxHash, TxKind, B256, U256};
use c_kzg::KzgSettings;
use core::fmt::Debug;
use reth_primitives_traits::{InMemorySize, SignedTransaction};
use reth_scroll_primitives::ScrollTransactionSigned;
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction, PoolTransaction,
};
use std::sync::{Arc, OnceLock};

/// Pool transaction for Scroll.
///
/// This type wraps the actual transaction and caches values that are frequently used by the pool.
/// For payload building this lazily tracks values that are required during payload building:
///  - Estimated compressed size of this transaction
#[derive(Debug, Clone, derive_more::Deref)]
pub struct ScrollPooledTransaction<
    Cons = ScrollTransactionSigned,
    Pooled = scroll_alloy_consensus::ScrollPooledTransaction,
> {
    #[deref]
    inner: EthPooledTransaction<Cons>,
    /// The pooled transaction type.
    _pd: core::marker::PhantomData<Pooled>,

    /// Cached EIP-2718 encoded bytes of the transaction, lazily computed.
    encoded_2718: OnceLock<Bytes>,
}

impl<Cons: SignedTransaction, Pooled> ScrollPooledTransaction<Cons, Pooled> {
    /// Create new instance of [Self].
    pub fn new(transaction: Recovered<Cons>, encoded_length: usize) -> Self {
        Self {
            inner: EthPooledTransaction::new(transaction, encoded_length),
            _pd: core::marker::PhantomData,
            encoded_2718: Default::default(),
        }
    }

    /// Returns lazily computed EIP-2718 encoded bytes of the transaction.
    pub fn encoded_2718(&self) -> &Bytes {
        self.encoded_2718.get_or_init(|| self.inner.transaction().encoded_2718().into())
    }
}

impl<Cons, Pooled> PoolTransaction for ScrollPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled>,
    Pooled: SignedTransaction + TryFrom<Cons, Error: core::error::Error>,
{
    type TryFromConsensusError = <Pooled as TryFrom<Cons>>::Error;
    type Consensus = Cons;
    type Pooled = Pooled;

    fn clone_into_consensus(&self) -> Recovered<Self::Consensus> {
        self.inner.transaction().clone()
    }

    fn into_consensus(self) -> Recovered<Self::Consensus> {
        self.inner.transaction
    }

    fn from_pooled(tx: Recovered<Self::Pooled>) -> Self {
        let encoded_len = tx.encode_2718_len();
        Self::new(tx.convert(), encoded_len)
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

impl<Cons: Typed2718, Pooled> Typed2718 for ScrollPooledTransaction<Cons, Pooled> {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl<Cons: InMemorySize, Pooled> InMemorySize for ScrollPooledTransaction<Cons, Pooled> {
    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl<Cons, Pooled> alloy_consensus::Transaction for ScrollPooledTransaction<Cons, Pooled>
where
    Cons: alloy_consensus::Transaction,
    Pooled: Debug + Send + Sync + 'static,
{
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

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.inner.authorization_list()
    }
}

impl<Cons, Pooled> EthPoolTransaction for ScrollPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled>,
    Pooled: SignedTransaction + TryFrom<Cons>,
    <Pooled as TryFrom<Cons>>::Error: core::error::Error,
{
    fn take_blob(&mut self) -> EthBlobTransactionSidecar {
        EthBlobTransactionSidecar::None
    }

    fn try_into_pooled_eip4844(
        self,
        _sidecar: Arc<BlobTransactionSidecarVariant>,
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
        _sidecar: &BlobTransactionSidecarVariant,
        _settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        Err(BlobTransactionValidationError::NotBlobTransaction(self.ty()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{ScrollPooledTransaction, ScrollTransactionValidator};
    use alloy_consensus::{transaction::Recovered, Signed};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::Signature;
    use reth_provider::test_utils::MockEthProvider;
    use reth_scroll_chainspec::SCROLL_MAINNET;
    use reth_scroll_primitives::ScrollTransactionSigned;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, validate::EthTransactionValidatorBuilder, TransactionOrigin,
        TransactionValidationOutcome,
    };
    use scroll_alloy_consensus::{ScrollTypedTransaction, TxL1Message};
    #[test]
    fn validate_scroll_transaction() {
        let client = MockEthProvider::default().with_chain_spec(SCROLL_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = ScrollTransactionValidator::new(validator);

        let origin = TransactionOrigin::External;
        let signer = Default::default();
        let deposit_tx = ScrollTypedTransaction::L1Message(TxL1Message::default());
        let signature = Signature::test_signature();
        let signed_tx: ScrollTransactionSigned = Signed::new_unhashed(deposit_tx, signature).into();
        let signed_recovered = Recovered::new_unchecked(signed_tx, signer);
        let len = signed_recovered.encode_2718_len();
        let pooled_tx: ScrollPooledTransaction =
            ScrollPooledTransaction::new(signed_recovered, len);
        let outcome = validator.validate_one(origin, pooled_tx);

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }
}
