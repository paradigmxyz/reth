use crate::{
    conditional::MaybeConditionalTransaction, estimated_da_size::DataAvailabilitySized,
    interop::MaybeInteropTransaction,
};
use alloy_consensus::{transaction::Recovered, BlobTransactionValidationError, Typed2718};
use alloy_eips::{
    eip2718::{Encodable2718, WithEncoded},
    eip2930::AccessList,
    eip7594::BlobTransactionSidecarVariant,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{Address, Bytes, TxHash, TxKind, B256, U256};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use c_kzg::KzgSettings;
use core::fmt::Debug;
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::{InMemorySize, SignedTransaction};
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction, PoolTransaction,
};
use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};

/// Marker for no-interop transactions
pub(crate) const NO_INTEROP_TX: u64 = 0;

/// Pool transaction for OP.
///
/// This type wraps the actual transaction and caches values that are frequently used by the pool.
/// For payload building this lazily tracks values that are required during payload building:
///  - Estimated compressed size of this transaction
#[derive(Debug, Clone, derive_more::Deref)]
pub struct OpPooledTransaction<
    Cons = OpTransactionSigned,
    Pooled = op_alloy_consensus::OpPooledTransaction,
> {
    #[deref]
    inner: EthPooledTransaction<Cons>,
    /// The estimated size of this transaction, lazily computed.
    estimated_tx_compressed_size: OnceLock<u64>,
    /// The pooled transaction type.
    _pd: core::marker::PhantomData<Pooled>,

    /// Optional conditional attached to this transaction.
    conditional: Option<Box<TransactionConditional>>,

    /// Optional interop deadline attached to this transaction.
    interop: Arc<AtomicU64>,

    /// Cached EIP-2718 encoded bytes of the transaction, lazily computed.
    encoded_2718: OnceLock<Bytes>,
}

impl<Cons: SignedTransaction, Pooled> OpPooledTransaction<Cons, Pooled> {
    /// Create new instance of [Self].
    pub fn new(transaction: Recovered<Cons>, encoded_length: usize) -> Self {
        Self {
            inner: EthPooledTransaction::new(transaction, encoded_length),
            estimated_tx_compressed_size: Default::default(),
            conditional: None,
            interop: Arc::new(AtomicU64::new(NO_INTEROP_TX)),
            _pd: core::marker::PhantomData,
            encoded_2718: Default::default(),
        }
    }

    /// Returns the estimated compressed size of a transaction in bytes.
    /// This value is computed based on the following formula:
    /// `max(minTransactionSize, intercept + fastlzCoef*fastlzSize) / 1e6`
    /// Uses cached EIP-2718 encoded bytes to avoid recomputing the encoding for each estimation.
    pub fn estimated_compressed_size(&self) -> u64 {
        *self
            .estimated_tx_compressed_size
            .get_or_init(|| op_alloy_flz::tx_estimated_size_fjord_bytes(self.encoded_2718()))
    }

    /// Returns lazily computed EIP-2718 encoded bytes of the transaction.
    pub fn encoded_2718(&self) -> &Bytes {
        self.encoded_2718.get_or_init(|| self.inner.transaction().encoded_2718().into())
    }

    /// Conditional setter.
    pub fn with_conditional(mut self, conditional: TransactionConditional) -> Self {
        self.conditional = Some(Box::new(conditional));
        self
    }
}

impl<Cons, Pooled> MaybeConditionalTransaction for OpPooledTransaction<Cons, Pooled> {
    fn set_conditional(&mut self, conditional: TransactionConditional) {
        self.conditional = Some(Box::new(conditional))
    }

    fn conditional(&self) -> Option<&TransactionConditional> {
        self.conditional.as_deref()
    }
}

impl<Cons, Pooled> MaybeInteropTransaction for OpPooledTransaction<Cons, Pooled> {
    fn set_interop_deadline(&self, deadline: u64) {
        self.interop.store(deadline, Ordering::Relaxed);
    }

    fn interop_deadline(&self) -> Option<u64> {
        let interop = self.interop.load(Ordering::Relaxed);
        if interop > NO_INTEROP_TX {
            return Some(interop)
        }
        None
    }
}

impl<Cons: SignedTransaction, Pooled> DataAvailabilitySized for OpPooledTransaction<Cons, Pooled> {
    fn estimated_da_size(&self) -> u64 {
        self.estimated_compressed_size()
    }
}

impl<Cons, Pooled> PoolTransaction for OpPooledTransaction<Cons, Pooled>
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

    fn into_consensus_with2718(self) -> WithEncoded<Recovered<Self::Consensus>> {
        let encoding = self.encoded_2718().clone();
        self.inner.transaction.into_encoded_with(encoding)
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

impl<Cons: Typed2718, Pooled> Typed2718 for OpPooledTransaction<Cons, Pooled> {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl<Cons: InMemorySize, Pooled> InMemorySize for OpPooledTransaction<Cons, Pooled> {
    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl<Cons, Pooled> alloy_consensus::Transaction for OpPooledTransaction<Cons, Pooled>
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

impl<Cons, Pooled> EthPoolTransaction for OpPooledTransaction<Cons, Pooled>
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

/// Helper trait to provide payload builder with access to conditionals and encoded bytes of
/// transaction.
pub trait OpPooledTx:
    MaybeConditionalTransaction + MaybeInteropTransaction + PoolTransaction + DataAvailabilitySized
{
    /// Returns the EIP-2718 encoded bytes of the transaction.
    fn encoded_2718(&self) -> Cow<'_, Bytes>;
}

impl<Cons, Pooled> OpPooledTx for OpPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled>,
    Pooled: SignedTransaction + TryFrom<Cons>,
    <Pooled as TryFrom<Cons>>::Error: core::error::Error,
{
    fn encoded_2718(&self) -> Cow<'_, Bytes> {
        Cow::Borrowed(self.encoded_2718())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        MetaTxDisabled, OpPooledTransaction, OpTransactionValidator, UnprotectedTxDisabled,
    };
    use alloy_consensus::{transaction::Recovered, SignableTransaction, TxEip1559, TxLegacy};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, Bytes, Signature, TxKind, U256};
    use op_alloy_consensus::TxDeposit;
    use reth_mantle_forks::MANTLE_META_TX_PREFIX;
    use reth_optimism_chainspec::{MANTLE_MAINNET, OP_MAINNET};
    use reth_optimism_primitives::OpTransactionSigned;
    use reth_provider::test_utils::MockEthProvider;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, error::PoolTransactionError,
        validate::EthTransactionValidatorBuilder, TransactionOrigin, TransactionValidationOutcome,
    };

    fn pooled_eip1559_with_input(input: Bytes) -> OpPooledTransaction {
        let signer = Address::ZERO;
        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: 5000,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input,
            ..Default::default()
        }
        .into_signed(Signature::new(U256::ZERO, U256::ZERO, false))
        .into();
        let signed_recovered = Recovered::new_unchecked(tx, signer);
        let len = signed_recovered.encode_2718_len();

        OpPooledTransaction::new(signed_recovered, len)
    }

    #[tokio::test]
    async fn validate_rejects_mantle_meta_tx_as_bad_transaction() {
        let client = MockEthProvider::default().with_chain_spec(MANTLE_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let mut input = MANTLE_META_TX_PREFIX.to_vec();
        input.push(0xF8);
        let outcome = validator
            .validate_one(TransactionOrigin::External, pooled_eip1559_with_input(input.into()))
            .await;

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid MetaTx"),
        };
        let meta_tx_err = err
            .downcast_other_ref::<MetaTxDisabled>()
            .expect("expected MetaTxDisabled txpool error");

        assert_eq!(err.to_string(), "meta tx is disabled");
        assert!(meta_tx_err.is_bad_transaction());
    }

    #[tokio::test]
    async fn validate_does_not_reject_exact_meta_tx_prefix_without_payload() {
        let client = MockEthProvider::default().with_chain_spec(MANTLE_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let outcome = validator
            .validate_one(
                TransactionOrigin::External,
                pooled_eip1559_with_input(MANTLE_META_TX_PREFIX.to_vec().into()),
            )
            .await;

        if let TransactionValidationOutcome::Invalid(_, err) = outcome {
            assert!(!err.is_other::<MetaTxDisabled>());
        }
    }

    #[tokio::test]
    async fn validate_optimism_transaction() {
        let client = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let origin = TransactionOrigin::External;
        let signer = Default::default();
        let deposit_tx = TxDeposit {
            source_hash: Default::default(),
            from: signer,
            to: TxKind::Create,
            mint: 0,
            value: U256::ZERO,
            gas_limit: 0,
            eth_tx_value: None,
            eth_value: 0,
            is_system_transaction: false,
            input: Default::default(),
        };
        let signed_tx: OpTransactionSigned = deposit_tx.into();
        let signed_recovered = Recovered::new_unchecked(signed_tx, signer);
        let len = signed_recovered.encode_2718_len();
        let pooled_tx: OpPooledTransaction = OpPooledTransaction::new(signed_recovered, len);
        let outcome = validator.validate_one(origin, pooled_tx).await;

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }

    /// Helper: create a legacy (type 0) pooled transaction.
    ///
    /// - `chain_id = Some(id)` → EIP-155 signed (replay protected)
    /// - `chain_id = None`     → non-EIP-155 / unprotected (v=27/28)
    fn pooled_legacy_tx(chain_id: Option<u64>) -> OpPooledTransaction {
        let signer = Address::ZERO;
        let tx: OpTransactionSigned = TxLegacy {
            chain_id,
            nonce: 0,
            gas_price: 1,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::default(),
        }
        .into_signed(Signature::new(U256::ZERO, U256::ZERO, false))
        .into();
        let signed_recovered = Recovered::new_unchecked(tx, signer);
        let len = signed_recovered.encode_2718_len();
        OpPooledTransaction::new(signed_recovered, len)
    }

    #[tokio::test]
    async fn validate_rejects_unprotected_legacy_tx() {
        // Non-EIP-155 legacy transaction (chain_id = None, equivalent to v=27/28)
        // should be rejected, matching op-geth's default AllowUnprotectedTxs=false behavior.
        let client = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let outcome =
            validator.validate_one(TransactionOrigin::External, pooled_legacy_tx(None)).await;

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected unprotected legacy tx to be rejected"),
        };
        let unprotected_err = err
            .downcast_other_ref::<UnprotectedTxDisabled>()
            .expect("expected UnprotectedTxDisabled txpool error");

        assert_eq!(err.to_string(), "only replay-protected (EIP-155) transactions allowed");
        assert!(unprotected_err.is_bad_transaction());
    }

    #[tokio::test]
    async fn validate_does_not_reject_eip155_legacy_tx() {
        // EIP-155 legacy transaction (chain_id = Some(10), matching OP mainnet chain ID)
        // should NOT be rejected by the unprotected-tx check.
        // It will fail later in stateful validation (balance/nonce), but the
        // UnprotectedTxDisabled error must not appear.
        let client = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let outcome =
            validator.validate_one(TransactionOrigin::External, pooled_legacy_tx(Some(10))).await;

        // Should not be rejected as UnprotectedTxDisabled
        if let TransactionValidationOutcome::Invalid(_, ref err) = outcome {
            assert!(
                !err.is_other::<UnprotectedTxDisabled>(),
                "EIP-155 legacy tx should not be rejected as unprotected: {err}"
            );
        }
    }

    #[tokio::test]
    async fn validate_eip1559_not_affected_by_unprotected_check() {
        // EIP-1559 (type 2) transactions always have chain_id and should not be
        // affected by the unprotected legacy tx check.
        let client = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let outcome = validator
            .validate_one(TransactionOrigin::External, pooled_eip1559_with_input(Bytes::default()))
            .await;

        // Should not be rejected as UnprotectedTxDisabled
        if let TransactionValidationOutcome::Invalid(_, ref err) = outcome {
            assert!(
                !err.is_other::<UnprotectedTxDisabled>(),
                "EIP-1559 tx should not be rejected as unprotected: {err}"
            );
        }
    }
}

/// Integration tests: verify that rejected/accepted transactions correctly affect Pool state.
///
/// Unlike the unit tests above (which call `validator.validate_one()` directly), these tests
/// construct a real `Pool<OpTransactionValidator<...>>` and submit transactions via
/// `pool.add_external_transaction()`, testing the full chain:
///   `pool.add_transaction()` → `TransactionValidator::validate_transaction()` → pool state update
#[cfg(test)]
mod pool_integration_tests {
    use crate::{OpPooledTransaction, OpTransactionValidator};
    use alloy_consensus::{transaction::Recovered, SignableTransaction, TxEip1559, TxLegacy};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, Bytes, Signature, TxKind, U256};
    use reth_mantle_forks::MANTLE_META_TX_PREFIX;
    use reth_optimism_chainspec::OP_MAINNET;
    use reth_optimism_primitives::OpTransactionSigned;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, validate::EthTransactionValidatorBuilder,
        CoinbaseTipOrdering, Pool, PoolConfig, PoolTransaction, TransactionPool,
    };

    /// Macro to build an OpTransactionValidator-backed Pool from a pre-configured provider.
    ///
    /// Avoids spelling out the full generic type of `Pool<OpTransactionValidator<...>>` which
    /// changes depending on the provider's `ChainSpec` parameter.
    macro_rules! build_pool {
        ($provider:expr) => {{
            let blob_store = InMemoryBlobStore::default();
            let eth_validator = EthTransactionValidatorBuilder::new($provider)
                .no_shanghai()
                .no_cancun()
                .build(blob_store.clone());
            let op_validator = OpTransactionValidator::new(eth_validator);
            Pool::new(
                op_validator,
                CoinbaseTipOrdering::default(),
                blob_store,
                PoolConfig::default(),
            )
        }};
    }

    fn pooled_legacy_tx(chain_id: Option<u64>) -> OpPooledTransaction {
        let signer = Address::ZERO;
        let tx: OpTransactionSigned = TxLegacy {
            chain_id,
            nonce: 0,
            gas_price: 7, // >= MIN_PROTOCOL_BASE_FEE
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::default(),
        }
        .into_signed(Signature::new(U256::ZERO, U256::ZERO, false))
        .into();
        let signed_recovered = Recovered::new_unchecked(tx, signer);
        let len = signed_recovered.encode_2718_len();
        OpPooledTransaction::new(signed_recovered, len)
    }

    fn pooled_eip1559(input: Bytes) -> OpPooledTransaction {
        let signer = Address::ZERO;
        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: 10,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas: 7, // >= MIN_PROTOCOL_BASE_FEE
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input,
            ..Default::default()
        }
        .into_signed(Signature::new(U256::ZERO, U256::ZERO, false))
        .into();
        let signed_recovered = Recovered::new_unchecked(tx, signer);
        let len = signed_recovered.encode_2718_len();
        OpPooledTransaction::new(signed_recovered, len)
    }

    #[tokio::test]
    async fn pool_rejects_unprotected_legacy_tx() {
        let provider = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let pool = build_pool!(provider);

        let tx = pooled_legacy_tx(None);
        let hash = *PoolTransaction::hash(&tx);
        let result = pool.add_external_transaction(tx).await;

        assert!(result.is_err(), "unprotected legacy tx should be rejected by pool");
        assert_eq!(pool.pool_size().total, 0, "rejected tx should not be in pool");
        assert!(pool.get(&hash).is_none(), "rejected tx should not be retrievable");
    }

    #[tokio::test]
    async fn pool_rejects_metatx() {
        let provider = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let pool = build_pool!(provider);

        let mut input = MANTLE_META_TX_PREFIX.to_vec();
        input.push(0xF8);
        let tx = pooled_eip1559(input.into());
        let hash = *PoolTransaction::hash(&tx);
        let result = pool.add_external_transaction(tx).await;

        assert!(result.is_err(), "MetaTx should be rejected by pool");
        assert_eq!(pool.pool_size().total, 0, "rejected MetaTx should not be in pool");
        assert!(pool.get(&hash).is_none(), "rejected MetaTx should not be retrievable");
    }

    #[tokio::test]
    async fn pool_accepts_eip155_legacy_tx() {
        let provider = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        // Fund the sender so balance/nonce checks pass
        provider.add_account(Address::ZERO, ExtendedAccount::new(0, U256::MAX));
        let pool = build_pool!(provider);

        let tx = pooled_legacy_tx(Some(10)); // OP mainnet chain_id = 10
        let hash = *PoolTransaction::hash(&tx);
        let result = pool.add_external_transaction(tx).await;

        assert!(result.is_ok(), "EIP-155 legacy tx should be accepted: {result:?}");
        assert!(pool.pool_size().pending >= 1, "accepted tx should be in pending pool");
        assert!(pool.get(&hash).is_some(), "accepted tx should be retrievable");
    }

    #[tokio::test]
    async fn pool_accepts_eip1559_tx() {
        let provider = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        provider.add_account(Address::ZERO, ExtendedAccount::new(0, U256::MAX));
        let pool = build_pool!(provider);

        let tx = pooled_eip1559(Bytes::default());
        let hash = *PoolTransaction::hash(&tx);
        let result = pool.add_external_transaction(tx).await;

        assert!(result.is_ok(), "EIP-1559 tx should be accepted: {result:?}");
        assert!(pool.pool_size().pending >= 1, "accepted tx should be in pending pool");
        assert!(pool.get(&hash).is_some(), "accepted tx should be retrievable");
    }
}
