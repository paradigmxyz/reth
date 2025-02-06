//! OP transaction pool types
use alloy_consensus::{
    BlobTransactionSidecar, BlobTransactionValidationError, BlockHeader, Transaction, Typed2718,
};
use alloy_eips::{eip2718::Encodable2718, eip7702::SignedAuthorization};
use alloy_primitives::{Address, Bytes, TxHash, TxKind, B256, U256};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use parking_lot::RwLock;
use reth_node_api::{Block, BlockBody};
use reth_optimism_evm::RethL1BlockInfo;
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives::{GotExpected, InvalidTransactionError, Recovered, SealedBlock};
use reth_primitives_traits::{InMemorySize, SignedTransaction};
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, StateProviderFactory};
use reth_revm::L1BlockInfo;
use reth_transaction_pool::{
    CoinbaseTipOrdering, EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction,
    EthTransactionValidator, Pool, PoolTransaction, TransactionOrigin,
    TransactionValidationOutcome, TransactionValidationTaskExecutor, TransactionValidator,
};
use revm::primitives::{AccessList, KzgSettings};
use std::{
    fmt::Debug,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};

/// Type alias for default optimism transaction pool
pub type OpTransactionPool<Client, S, T = OpPooledTransaction> = Pool<
    TransactionValidationTaskExecutor<OpTransactionValidator<Client, T>>,
    CoinbaseTipOrdering<T>,
    S,
>;

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
}

impl<Cons: SignedTransaction, Pooled> OpPooledTransaction<Cons, Pooled> {
    /// Create new instance of [Self].
    pub fn new(transaction: Recovered<Cons>, encoded_length: usize) -> Self {
        Self {
            inner: EthPooledTransaction::new(transaction, encoded_length),
            estimated_tx_compressed_size: Default::default(),
            conditional: None,
            _pd: core::marker::PhantomData,
        }
    }

    /// Returns the estimated compressed size of a transaction in bytes scaled by 1e6.
    /// This value is computed based on the following formula:
    /// `max(minTransactionSize, intercept + fastlzCoef*fastlzSize)`
    pub fn estimated_compressed_size(&self) -> u64 {
        *self.estimated_tx_compressed_size.get_or_init(|| {
            op_alloy_flz::tx_estimated_size_fjord(&self.inner.transaction().encoded_2718())
        })
    }

    /// Conditional setter.
    pub fn with_conditional(mut self, conditional: TransactionConditional) -> Self {
        self.conditional = Some(Box::new(conditional));
        self
    }

    /// Conditional getter.
    pub fn conditional(&self) -> Option<&TransactionConditional> {
        self.conditional.as_deref()
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

    fn from_pooled(tx: Recovered<Self::Pooled>) -> Self {
        let encoded_len = tx.encode_2718_len();
        let tx = tx.map_transaction(|tx| tx.into());
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
    fn chain_id(&self) -> Option<alloy_primitives::ChainId> {
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
        _sidecar: Arc<BlobTransactionSidecar>,
    ) -> Option<Recovered<Self::Pooled>> {
        None
    }

    fn try_from_eip4844(
        _tx: Recovered<Self::Consensus>,
        _sidecar: BlobTransactionSidecar,
    ) -> Option<Self> {
        None
    }

    fn validate_blob(
        &self,
        _sidecar: &BlobTransactionSidecar,
        _settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        Err(BlobTransactionValidationError::NotBlobTransaction(self.ty()))
    }
}

/// Validator for Optimism transactions.
#[derive(Debug, Clone)]
pub struct OpTransactionValidator<Client, Tx> {
    /// The type that performs the actual validation.
    inner: EthTransactionValidator<Client, Tx>,
    /// Additional block info required for validation.
    block_info: Arc<OpL1BlockInfo>,
    /// If true, ensure that the transaction's sender has enough balance to cover the L1 gas fee
    /// derived from the tracked L1 block info that is extracted from the first transaction in the
    /// L2 block.
    require_l1_data_gas_fee: bool,
}

impl<Client, Tx> OpTransactionValidator<Client, Tx> {
    /// Returns the configured chain spec
    pub fn chain_spec(&self) -> Arc<Client::ChainSpec>
    where
        Client: ChainSpecProvider,
    {
        self.inner.chain_spec()
    }

    /// Returns the configured client
    pub fn client(&self) -> &Client {
        self.inner.client()
    }

    /// Returns the current block timestamp.
    fn block_timestamp(&self) -> u64 {
        self.block_info.timestamp.load(Ordering::Relaxed)
    }

    /// Returns the current block number.
    fn block_number(&self) -> u64 {
        self.block_info.number.load(Ordering::Relaxed)
    }

    /// Whether to ensure that the transaction's sender has enough balance to also cover the L1 gas
    /// fee.
    pub fn require_l1_data_gas_fee(self, require_l1_data_gas_fee: bool) -> Self {
        Self { require_l1_data_gas_fee, ..self }
    }

    /// Returns whether this validator also requires the transaction's sender to have enough balance
    /// to cover the L1 gas fee.
    pub const fn requires_l1_data_gas_fee(&self) -> bool {
        self.require_l1_data_gas_fee
    }
}

impl<Client, Tx> OpTransactionValidator<Client, Tx>
where
    Client: ChainSpecProvider<ChainSpec: OpHardforks> + StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction,
{
    /// Create a new [`OpTransactionValidator`].
    pub fn new(inner: EthTransactionValidator<Client, Tx>) -> Self {
        let this = Self::with_block_info(inner, OpL1BlockInfo::default());
        if let Ok(Some(block)) =
            this.inner.client().block_by_number_or_tag(alloy_eips::BlockNumberOrTag::Latest)
        {
            // genesis block has no txs, so we can't extract L1 info, we set the block info to empty
            // so that we will accept txs into the pool before the first block
            if block.header().number() == 0 {
                this.block_info.timestamp.store(block.header().timestamp(), Ordering::Relaxed);
                this.block_info.number.store(block.header().number(), Ordering::Relaxed);
            } else {
                this.update_l1_block_info(block.header(), block.body().transactions().first());
            }
        }

        this
    }

    /// Create a new [`OpTransactionValidator`] with the given [`OpL1BlockInfo`].
    pub fn with_block_info(
        inner: EthTransactionValidator<Client, Tx>,
        block_info: OpL1BlockInfo,
    ) -> Self {
        Self { inner, block_info: Arc::new(block_info), require_l1_data_gas_fee: true }
    }

    /// Update the L1 block info for the given header and system transaction, if any.
    ///
    /// Note: this supports optional system transaction, in case this is used in a dev setuo
    pub fn update_l1_block_info<H, T>(&self, header: &H, tx: Option<&T>)
    where
        H: BlockHeader,
        T: Transaction,
    {
        self.block_info.timestamp.store(header.timestamp(), Ordering::Relaxed);
        self.block_info.number.store(header.number(), Ordering::Relaxed);

        if let Some(Ok(cost_addition)) = tx.map(reth_optimism_evm::extract_l1_info_from_tx) {
            *self.block_info.l1_block_info.write() = cost_addition;
        }
    }

    /// Validates a single transaction.
    ///
    /// See also [`TransactionValidator::validate_transaction`]
    ///
    /// This behaves the same as [`EthTransactionValidator::validate_one`], but in addition, ensures
    /// that the account has enough balance to cover the L1 gas cost.
    pub fn validate_one(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
    ) -> TransactionValidationOutcome<Tx> {
        if transaction.is_eip4844() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TxTypeNotSupported.into(),
            )
        }

        let outcome = self.inner.validate_one(origin, transaction);

        if !self.requires_l1_data_gas_fee() {
            // no need to check L1 gas fee
            return outcome
        }

        // ensure that the account has enough balance to cover the L1 gas cost
        if let TransactionValidationOutcome::Valid {
            balance,
            state_nonce,
            transaction: valid_tx,
            propagate,
        } = outcome
        {
            let mut l1_block_info = self.block_info.l1_block_info.read().clone();

            let mut encoded = Vec::with_capacity(valid_tx.transaction().encoded_length());
            let tx = valid_tx.transaction().clone_into_consensus();
            tx.encode_2718(&mut encoded);

            let cost_addition = match l1_block_info.l1_tx_data_fee(
                self.chain_spec(),
                self.block_timestamp(),
                self.block_number(),
                &encoded,
                false,
            ) {
                Ok(cost) => cost,
                Err(err) => {
                    return TransactionValidationOutcome::Error(*valid_tx.hash(), Box::new(err))
                }
            };
            let cost = valid_tx.transaction().cost().saturating_add(cost_addition);

            // Checks for max cost
            if cost > balance {
                return TransactionValidationOutcome::Invalid(
                    valid_tx.into_transaction(),
                    InvalidTransactionError::InsufficientFunds(
                        GotExpected { got: balance, expected: cost }.into(),
                    )
                    .into(),
                )
            }

            return TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction: valid_tx,
                propagate,
            }
        }

        outcome
    }

    /// Validates all given transactions.
    ///
    /// Returns all outcomes for the given transactions in the same order.
    ///
    /// See also [`Self::validate_one`]
    pub fn validate_all(
        &self,
        transactions: Vec<(TransactionOrigin, Tx)>,
    ) -> Vec<TransactionValidationOutcome<Tx>> {
        transactions.into_iter().map(|(origin, tx)| self.validate_one(origin, tx)).collect()
    }
}

impl<Client, Tx> TransactionValidator for OpTransactionValidator<Client, Tx>
where
    Client: ChainSpecProvider<ChainSpec: OpHardforks> + StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction,
{
    type Transaction = Tx;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        self.validate_one(origin, transaction)
    }

    async fn validate_transactions(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        self.validate_all(transactions)
    }

    fn on_new_head_block<B>(&self, new_tip_block: &SealedBlock<B>)
    where
        B: Block,
    {
        self.inner.on_new_head_block(new_tip_block);
        self.update_l1_block_info(
            new_tip_block.header(),
            new_tip_block.body().transactions().first(),
        );
    }
}

/// Tracks additional infos for the current block.
#[derive(Debug, Default)]
pub struct OpL1BlockInfo {
    /// The current L1 block info.
    l1_block_info: RwLock<L1BlockInfo>,
    /// Current block timestamp.
    timestamp: AtomicU64,
    /// Current block number.
    number: AtomicU64,
}

#[cfg(test)]
mod tests {
    use crate::txpool::{OpPooledTransaction, OpTransactionValidator};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{PrimitiveSignature as Signature, TxKind, U256};
    use op_alloy_consensus::{OpTypedTransaction, TxDeposit};
    use reth_optimism_chainspec::OP_MAINNET;
    use reth_optimism_primitives::OpTransactionSigned;
    use reth_primitives::Recovered;
    use reth_provider::test_utils::MockEthProvider;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, validate::EthTransactionValidatorBuilder, TransactionOrigin,
        TransactionValidationOutcome,
    };
    #[test]
    fn validate_optimism_transaction() {
        let client = MockEthProvider::default().with_chain_spec(OP_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let origin = TransactionOrigin::External;
        let signer = Default::default();
        let deposit_tx = OpTypedTransaction::Deposit(TxDeposit {
            source_hash: Default::default(),
            from: signer,
            to: TxKind::Create,
            mint: None,
            value: U256::ZERO,
            gas_limit: 0,
            is_system_transaction: false,
            input: Default::default(),
        });
        let signature = Signature::test_signature();
        let signed_tx = OpTransactionSigned::new_unhashed(deposit_tx, signature);
        let signed_recovered = Recovered::new_unchecked(signed_tx, signer);
        let len = signed_recovered.encode_2718_len();
        let pooled_tx: OpPooledTransaction = OpPooledTransaction::new(signed_recovered, len);
        let outcome = validator.validate_one(origin, pooled_tx);

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }
}
