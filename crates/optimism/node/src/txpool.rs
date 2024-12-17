//! OP transaction pool types
use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip2718::Encodable2718;
use parking_lot::RwLock;
use reth_chainspec::ChainSpec;
use reth_node_api::{Block, BlockBody};
use reth_optimism_evm::RethL1BlockInfo;
use reth_primitives::{GotExpected, InvalidTransactionError, SealedBlock, TransactionSigned};
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use reth_revm::L1BlockInfo;
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
    /// If true, ensure that the transaction's sender has enough balance to cover the L1 gas fee
    /// derived from the tracked L1 block info that is extracted from the first transaction in the
    /// L2 block.
    require_l1_data_gas_fee: bool,
}

impl<Client, Tx> OpTransactionValidator<Client, Tx> {
    /// Returns the configured chain spec
    pub fn chain_spec(&self) -> &Arc<ChainSpec> {
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
    Client: StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction<Consensus = TransactionSigned>,
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
            let l1_block_info = self.block_info.l1_block_info.read().clone();

            let mut encoded = Vec::with_capacity(valid_tx.transaction().encoded_length());
            let tx = valid_tx.transaction().clone_into_consensus();
            tx.encode_2718(&mut encoded);

            let cost_addition = match l1_block_info.l1_tx_data_fee(
                self.chain_spec(),
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
    Client: StateProviderFactory + BlockReaderIdExt<Block = reth_primitives::Block>,
    Tx: EthPoolTransaction<Consensus = TransactionSigned>,
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

    fn on_new_head_block<H, B>(&self, new_tip_block: &SealedBlock<H, B>)
    where
        H: reth_primitives_traits::BlockHeader,
        B: BlockBody,
    {
        self.inner.on_new_head_block(new_tip_block);
        self.update_l1_block_info(
            new_tip_block.header(),
            new_tip_block.body.transactions().first(),
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
}

#[cfg(test)]
mod tests {
    use crate::txpool::OpTransactionValidator;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{PrimitiveSignature as Signature, TxKind, U256};
    use op_alloy_consensus::TxDeposit;
    use reth_chainspec::MAINNET;
    use reth_primitives::{RecoveredTx, Transaction, TransactionSigned};
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
            to: TxKind::Create,
            mint: None,
            value: U256::ZERO,
            gas_limit: 0,
            is_system_transaction: false,
            input: Default::default(),
        });
        let signature = Signature::test_signature();
        let signed_tx = TransactionSigned::new_unhashed(deposit_tx, signature);
        let signed_recovered = RecoveredTx::from_signed_transaction(signed_tx, signer);
        let len = signed_recovered.encode_2718_len();
        let pooled_tx = EthPooledTransaction::new(signed_recovered, len);
        let outcome = validator.validate_one(origin, pooled_tx);

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }
}
