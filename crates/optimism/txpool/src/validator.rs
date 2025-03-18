use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::Encodable2718;
use op_revm::L1BlockInfo;
use parking_lot::RwLock;
use reth_chainspec::ChainSpecProvider;
use reth_optimism_evm::RethL1BlockInfo;
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::supervisor::{
    ExecutingDescriptor, InteropTxValidator, SafetyLevel, SupervisorClient,
};
use reth_primitives_traits::{
    transaction::error::InvalidTransactionError, Block, BlockBody, GotExpected, SealedBlock,
};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, EthPoolTransaction, EthTransactionValidator,
    TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tracing::trace;

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
    /// Client used to check transaction validity with op-supervisor
    supervisor_client: Option<SupervisorClient>,
    /// Supervisor safety level
    supervisor_safety_level: SafetyLevel,
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
        Self {
            inner,
            block_info: Arc::new(block_info),
            require_l1_data_gas_fee: true,
            supervisor_client: None,
            supervisor_safety_level: SafetyLevel::CrossUnsafe,
        }
    }

    /// Set the supervisor client and safety level
    pub fn with_supervisor(
        mut self,
        supervisor_client: Option<SupervisorClient>,
        safety_level: SafetyLevel,
    ) -> Self {
        self.supervisor_client = supervisor_client;
        self.supervisor_safety_level = safety_level;
        self
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

        // Interop cross tx validation
        if self.is_valid_cross_tx(&transaction) == Some(false) {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::CrossTxInvalid,
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

    /// Extracts commitment from access list entries, pointing to 0x420..022 and validates them
    /// against supervisor.
    ///
    /// If commitment present pre-interop tx rejected.
    pub fn is_valid_cross_tx(&self, tx: &Tx) -> Option<bool> {
        // We don't need to check for deposit transaction in here, because they won't come from
        // txpool
        let access_list = tx.access_list()?;
        let inbox_entries =
            SupervisorClient::parse_access_list(access_list).copied().collect::<Vec<_>>();
        if inbox_entries.is_empty() {
            return None;
        }
        let timestamp = self.block_info.timestamp.load(Ordering::Relaxed);
        // Interop check
        if !self.chain_spec().is_interop_active_at_timestamp(timestamp) {
            // No cross chain tx allowed before interop
            return Some(false)
        }
        let client = self
            .supervisor_client
            .as_ref()
            .expect("supervisor client should be always set after interop is active");
        let safety_level = self.supervisor_safety_level;
        match tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async {
                client
                    .validate_messages(
                        inbox_entries.as_slice(),
                        safety_level,
                        ExecutingDescriptor::new(timestamp, Some(86400)),
                        Some(core::time::Duration::from_millis(100)),
                    )
                    .await
            })
        }) {
            Ok(()) => Some(true),
            Err(_) => {
                trace!(target: "txpool", hash=%tx.hash(), "Cross chain transaction invalid");
                Some(false)
            }
        }
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
