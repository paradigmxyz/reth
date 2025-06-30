use alloy_consensus::BlockHeader;
use alloy_eips::Encodable2718;
use parking_lot::RwLock;
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::{
    transaction::error::InvalidTransactionError, Block, GotExpected, SealedBlock,
};
use reth_revm::database::StateProviderDatabase;
use reth_scroll_evm::{
    compute_compression_ratio, spec_id_at_timestamp_and_number, RethL1BlockInfo,
};
use reth_scroll_forks::ScrollHardforks;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{
    EthPoolTransaction, EthTransactionValidator, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator,
};
use revm_scroll::l1block::L1BlockInfo;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Tracks additional infos for the current block.
#[derive(Debug, Default)]
pub struct ScrollL1BlockInfo {
    /// The current L1 block info.
    l1_block_info: RwLock<L1BlockInfo>,
    /// Current block timestamp.
    timestamp: AtomicU64,
    /// Current block number.
    number: AtomicU64,
}

/// Validator for Scroll transactions.
#[derive(Debug, Clone)]
pub struct ScrollTransactionValidator<Client, Tx> {
    /// The type that performs the actual validation.
    inner: EthTransactionValidator<Client, Tx>,
    /// Additional block info required for validation.
    block_info: Arc<ScrollL1BlockInfo>,
    /// If true, ensure that the transaction's sender has enough balance to cover the L1 gas fee
    /// derived from the tracked L1 block info.
    require_l1_data_gas_fee: bool,
}

impl<Client, Tx> ScrollTransactionValidator<Client, Tx> {
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

impl<Client, Tx> ScrollTransactionValidator<Client, Tx>
where
    Client: ChainSpecProvider<ChainSpec: ScrollHardforks> + StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction,
{
    /// Create a new [`ScrollTransactionValidator`].
    pub fn new(inner: EthTransactionValidator<Client, Tx>) -> Self {
        let this = Self::with_block_info(inner, ScrollL1BlockInfo::default());
        if let Ok(Some(block)) =
            this.inner.client().block_by_number_or_tag(alloy_eips::BlockNumberOrTag::Latest)
        {
            this.block_info.timestamp.store(block.header().timestamp(), Ordering::Relaxed);
            this.block_info.number.store(block.header().number(), Ordering::Relaxed);
            this.update_l1_block_info(block.header());
        }

        this
    }

    /// Create a new [`ScrollTransactionValidator`] with the given [`ScrollL1BlockInfo`].
    pub fn with_block_info(
        inner: EthTransactionValidator<Client, Tx>,
        block_info: ScrollL1BlockInfo,
    ) -> Self {
        Self { inner, block_info: Arc::new(block_info), require_l1_data_gas_fee: true }
    }

    /// Update the L1 block info for the given header and system transaction, if any.
    pub fn update_l1_block_info<H>(&self, header: &H)
    where
        H: BlockHeader,
    {
        self.block_info.timestamp.store(header.timestamp(), Ordering::Relaxed);
        self.block_info.number.store(header.number(), Ordering::Relaxed);

        let provider =
            self.client().state_by_block_number_or_tag(header.number().into()).expect("msg");
        let mut db = StateProviderDatabase::new(provider);
        let spec_id =
            spec_id_at_timestamp_and_number(header.timestamp(), header.number(), self.chain_spec());
        if let Ok(l1_block_info) = L1BlockInfo::try_fetch(&mut db, spec_id) {
            *self.block_info.l1_block_info.write() = l1_block_info;
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
            bytecode_hash,
            authorities,
        } = outcome
        {
            let mut l1_block_info = self.block_info.l1_block_info.read().clone();

            let mut encoded = Vec::with_capacity(valid_tx.transaction().encoded_length());
            let tx = valid_tx.transaction().clone_into_consensus();
            tx.encode_2718(&mut encoded);
            let compression_ratio = compute_compression_ratio(valid_tx.transaction().input());

            let cost_addition = match l1_block_info.l1_tx_data_fee(
                self.chain_spec(),
                self.block_timestamp(),
                self.block_number(),
                &encoded,
                Some(compression_ratio),
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
                bytecode_hash,
                transaction: valid_tx,
                propagate,
                authorities,
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

impl<Client, Tx> TransactionValidator for ScrollTransactionValidator<Client, Tx>
where
    Client: ChainSpecProvider<ChainSpec: ScrollHardforks> + StateProviderFactory + BlockReaderIdExt,
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
        self.update_l1_block_info(new_tip_block.header());
    }
}
