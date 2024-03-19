//! OP transaction pool types
use parking_lot::RwLock;
use reth_primitives::{Block, ChainSpec, GotExpected, InvalidTransactionError, SealedBlock};
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use reth_revm::{optimism::RethL1BlockInfo, L1BlockInfo};
use reth_transaction_pool::{
    CoinbaseTipOrdering, EthPoolTransaction, EthPooledTransaction, EthTransactionValidator, Pool,
    TransactionOrigin, TransactionValidationOutcome, TransactionValidationTaskExecutor,
    TransactionValidator,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Type alias for default optimism transaction pool
pub type OpTransactionPool<Client, S> = Pool<
    TransactionValidationTaskExecutor<OpTransactionValidator<Client, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    S,
>;

/// Validator for Optimism transactions.
#[derive(Debug, Clone)]
pub struct OpTransactionValidator<Client, Tx> {
    /// The type that performs the actual validation.
    inner: EthTransactionValidator<Client, Tx>,
    /// Additional block info required for validation.
    block_info: Arc<OpL1BlockInfo>,
}

impl<Client, Tx> OpTransactionValidator<Client, Tx> {
    /// Returns the configured chain spec
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.inner.chain_spec()
    }

    /// Returns the current block timestamp.
    fn block_timestamp(&self) -> u64 {
        self.block_info.timestamp.load(Ordering::Relaxed)
    }
}

impl<Client, Tx> OpTransactionValidator<Client, Tx>
where
    Client: StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction,
{
    /// Create a new [OpTransactionValidator].
    pub fn new(inner: EthTransactionValidator<Client, Tx>) -> Self {
        let this = Self::with_block_info(inner, OpL1BlockInfo::default());
        if let Ok(Some(block)) =
            this.inner.client().block_by_number_or_tag(reth_primitives::BlockNumberOrTag::Latest)
        {
            this.update_l1_block_info(&block);
        }

        this
    }

    /// Create a new [OpTransactionValidator] with the given [OpL1BlockInfo].
    pub fn with_block_info(
        inner: EthTransactionValidator<Client, Tx>,
        block_info: OpL1BlockInfo,
    ) -> Self {
        Self { inner, block_info: Arc::new(block_info) }
    }

    /// Update the L1 block info.
    fn update_l1_block_info(&self, block: &Block) {
        self.block_info.timestamp.store(block.timestamp, Ordering::Relaxed);
        let cost_addition = reth_revm::optimism::extract_l1_info(block).ok();
        *self.block_info.l1_block_info.write() = cost_addition;
    }

    /// Validates a single transaction.
    ///
    /// See also [TransactionValidator::validate_transaction]
    ///
    /// This behaves the same as [EthTransactionValidator::validate_one], but in addition, ensures
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

        // ensure that the account has enough balance to cover the L1 gas cost
        if let TransactionValidationOutcome::Valid {
            balance,
            state_nonce,
            transaction: valid_tx,
            propagate,
        } = outcome
        {
            let Some(l1_block_info) = self.block_info.l1_block_info.read().clone() else {
                return TransactionValidationOutcome::Error(
                    *valid_tx.hash(),
                    "L1BlockInfoError".into(),
                )
            };

            let mut encoded = Vec::new();
            valid_tx.transaction().to_recovered_transaction().encode_enveloped(&mut encoded);

            let cost_addition = match l1_block_info.l1_tx_data_fee(
                &self.chain_spec(),
                self.block_timestamp(),
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
    /// See also [Self::validate_one]
    pub fn validate_all(
        &self,
        transactions: Vec<(TransactionOrigin, Tx)>,
    ) -> Vec<TransactionValidationOutcome<Tx>> {
        transactions.into_iter().map(|(origin, tx)| self.validate_one(origin, tx)).collect()
    }
}

impl<Client, Tx> TransactionValidator for OpTransactionValidator<Client, Tx>
where
    Client: StateProviderFactory + BlockReaderIdExt,
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

    fn on_new_head_block(&self, new_tip_block: &SealedBlock) {
        self.inner.on_new_head_block(new_tip_block);
        self.update_l1_block_info(&new_tip_block.clone().unseal());
    }
}

/// Tracks additional infos for the current block.
#[derive(Debug, Default)]
pub struct OpL1BlockInfo {
    /// The current L1 block info.
    l1_block_info: RwLock<Option<L1BlockInfo>>,
    /// Current block timestamp.
    timestamp: AtomicU64,
}

#[cfg(test)]
mod tests {
    use crate::txpool::OpTransactionValidator;
    use reth_primitives::{
        Signature, Transaction, TransactionKind, TransactionSigned, TransactionSignedEcRecovered,
        TxDeposit, MAINNET, U256,
    };
    use reth_provider::test_utils::MockEthProvider;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, validate::EthTransactionValidatorBuilder,
        EthPooledTransaction, TransactionOrigin, TransactionValidationOutcome,
    };

    #[test]
    fn validate_optimism_transaction() {
        let client = MockEthProvider::default();
        let validator = EthTransactionValidatorBuilder::new(MAINNET.clone())
            .no_shanghai()
            .no_cancun()
            .build(client, InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let origin = TransactionOrigin::External;
        let signer = Default::default();
        let deposit_tx = Transaction::Deposit(TxDeposit {
            source_hash: Default::default(),
            from: signer,
            to: TransactionKind::Create,
            mint: None,
            value: U256::ZERO,
            gas_limit: 0u64,
            is_system_transaction: false,
            input: Default::default(),
        });
        let signature = Signature::default();
        let signed_tx = TransactionSigned::from_transaction_and_signature(deposit_tx, signature);
        let signed_recovered =
            TransactionSignedEcRecovered::from_signed_transaction(signed_tx, signer);
        let len = signed_recovered.length_without_header();
        let pooled_tx = EthPooledTransaction::new(signed_recovered, len);
        let outcome = validator.validate_one(origin, pooled_tx);

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }
}
