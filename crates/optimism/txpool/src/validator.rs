use crate::{supervisor::SupervisorClient, InvalidCrossTx, OpPooledTx};
use alloy_consensus::{BlockHeader, Transaction};
use futures_util::future;
use op_revm::L1BlockInfo;
use parking_lot::RwLock;
use reth_chainspec::ChainSpecProvider;
use reth_optimism_evm::RethL1BlockInfo;
use reth_optimism_forks::OpHardforks;
use reth_primitives_traits::{
    transaction::error::InvalidTransactionError, Block, BlockBody, GotExpected, SealedBlock,
};
use reth_storage_api::{AccountInfoReader, BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, EthPoolTransaction, EthTransactionValidator,
    TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

/// The interval for which we check transaction against supervisor, 1 hour.
const TRANSACTION_VALIDITY_WINDOW_SECS: u64 = 3600;

/// Tracks additional infos for the current block.
#[derive(Debug, Default)]
pub struct OpL1BlockInfo {
    /// The current L1 block info.
    l1_block_info: RwLock<L1BlockInfo>,
    /// Current block timestamp.
    timestamp: AtomicU64,
}

impl OpL1BlockInfo {
    /// Returns the most recent timestamp
    pub fn timestamp(&self) -> u64 {
        self.timestamp.load(Ordering::Relaxed)
    }
}

/// Validator for Optimism transactions.
#[derive(Debug, Clone)]
pub struct OpTransactionValidator<Client, Tx> {
    /// The type that performs the actual validation.
    inner: Arc<EthTransactionValidator<Client, Tx>>,
    /// Additional block info required for validation.
    block_info: Arc<OpL1BlockInfo>,
    /// If true, ensure that the transaction's sender has enough balance to cover the L1 gas fee
    /// derived from the tracked L1 block info that is extracted from the first transaction in the
    /// L2 block.
    require_l1_data_gas_fee: bool,
    /// Client used to check transaction validity with op-supervisor
    supervisor_client: Option<SupervisorClient>,
    /// tracks activated forks relevant for transaction validation
    fork_tracker: Arc<OpForkTracker>,
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
    Tx: EthPoolTransaction + OpPooledTx,
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
        Self {
            inner: Arc::new(inner),
            block_info: Arc::new(block_info),
            require_l1_data_gas_fee: true,
            supervisor_client: None,
            fork_tracker: Arc::new(OpForkTracker { interop: AtomicBool::from(false) }),
        }
    }

    /// Set the supervisor client and safety level
    pub fn with_supervisor(mut self, supervisor_client: SupervisorClient) -> Self {
        self.supervisor_client = Some(supervisor_client);
        self
    }

    /// Update the L1 block info for the given header and system transaction, if any.
    ///
    /// Note: this supports optional system transaction, in case this is used in a dev setup
    pub fn update_l1_block_info<H, T>(&self, header: &H, tx: Option<&T>)
    where
        H: BlockHeader,
        T: Transaction,
    {
        self.block_info.timestamp.store(header.timestamp(), Ordering::Relaxed);

        if let Some(Ok(l1_block_info)) = tx.map(reth_optimism_evm::extract_l1_info_from_tx) {
            *self.block_info.l1_block_info.write() = l1_block_info;
        }

        if self.chain_spec().is_interop_active_at_timestamp(header.timestamp()) {
            self.fork_tracker.interop.store(true, Ordering::Relaxed);
        }
    }

    /// Validates a single transaction.
    ///
    /// Costly because it creates a new state provider internally. For batch validation use
    /// [`validate_all`](Self::validate_all).
    ///
    /// This behaves the same as [`EthTransactionValidator::validate_one_with_state`], but in
    /// addition applies OP validity checks:
    /// - ensures tx is not eip4844
    /// - ensures that the account has enough balance to cover the L1 gas cost
    /// - ensures cross chain transactions are valid wrt locally configured safety level and
    ///   superchain state
    pub async fn validate_one(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
    ) -> TransactionValidationOutcome<Tx> {
        // stateless checks
        let transaction = match self.apply_checks_no_state(origin, transaction) {
            Ok(tx) => tx,
            Err(invalid_tx) => return invalid_tx,
        };
        // loads state provider
        let state = match self.client().latest() {
            Ok(s) => s,
            Err(err) => {
                return TransactionValidationOutcome::Error(*transaction.hash(), Box::new(err))
            }
        };
        // checks against state
        let outcomes = self.apply_checks_against_state(origin, transaction, state);
        // checks against superstate
        self.apply_checks_against_superchain_state(outcomes).await
    }

    /// Validates all given transactions.
    ///
    /// Returns all outcomes for the given transactions in the same order.
    ///
    /// Difference to [`Self::validate_one`] is that this method uses one and the same state
    /// provider to validate all given transactions, making it less costly for batch validation.
    pub async fn validate_all(
        &self,
        transactions: Vec<(TransactionOrigin, Tx)>,
    ) -> Vec<TransactionValidationOutcome<Tx>> {
        // checks that don't require state
        let transactions = transactions
            .into_iter()
            .map(|(origin, tx)| self.apply_checks_no_state(origin, tx).map(|tx| (origin, tx)))
            .collect::<Vec<_>>();

        // bail early if all transactions failed validation without state
        let some_pass = transactions.iter().any(|res| res.is_ok());
        if !some_pass {
            return transactions.into_iter().filter_map(|res| res.err()).collect()
        }

        // load state from DB for checks against state
        let state = match self.client().latest() {
            Ok(s) => s,
            Err(err) => {
                return transactions
                    .into_iter()
                    .map(|res| match res {
                        Ok((_, tx)) => {
                            TransactionValidationOutcome::Error(*tx.hash(), Box::new(err.clone()))
                        }
                        Err(already_invalid) => already_invalid,
                    })
                    .collect()
            }
        };
        // checks against state
        let transactions = transactions
            .into_iter()
            .map(|res| match res {
                Ok((origin, tx)) => self.apply_checks_against_state(origin, tx, &state),
                Err(invalid_outcome) => invalid_outcome,
            })
            .collect::<Vec<_>>();

        // checks against superstate
        future::join_all(
            transactions.into_iter().map(|res| self.apply_checks_against_superchain_state(res)),
        )
        .await
    }

    /// Performs validation, not requiring chain state, on single transaction.
    ///
    /// Returns unaltered input transaction if all checks pass, so transaction can continue
    /// through to stateful validation as argument to
    /// [`Self::apply_checks_against_state`]. Failed checks
    /// return [`TransactionValidationOutcome::Invalid`] wrapping the transaction or
    /// [`TransactionValidationOutcome::Error`].
    ///
    /// Under the hood calls OP specific checks not requiring chain state
    /// [`apply_op_checks_no_state`](Self::apply_op_checks_no_state), then checks inherited from
    /// L1 that don't require chain state
    /// [`EthTransactionValidator::apply_checks_no_state`].
    pub fn apply_checks_no_state(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
    ) -> Result<Tx, TransactionValidationOutcome<Tx>> {
        // OP checks without state
        let transaction = self.apply_op_checks_no_state(transaction)?;
        // checks inherited from L1
        self.inner.apply_checks_no_state(origin, transaction)
    }

    /// Validates single transaction using given state.
    ///
    /// Under the hood calls checks against chain state inherited from L1
    /// [`EthTransactionValidator::apply_checks_against_state`], then
    /// OP specific checks against chain state
    /// [`apply_op_checks_against_state`](Self::apply_op_checks_against_state).
    pub fn apply_checks_against_state<P>(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
        state: P,
    ) -> TransactionValidationOutcome<Tx>
    where
        P: AccountInfoReader,
    {
        // checks inherited from L1
        let l1_validation_outcome =
            self.inner.apply_checks_against_state(origin, transaction, state);
        // OP checks against state bundled in L1 validation outcome and then superchain state (by
        // RPC call to supervisor).
        self.apply_op_checks_against_state(l1_validation_outcome)
    }

    /// Applies OP validity checks, that do _not_ require reading latest state from DB (or from
    /// Superchain oracle), to single transaction.
    ///
    /// Returns the unmodified transaction if all checks pass, ready to be passed onto the next set
    /// of checks in the validation pipeline, namely the checks not requiring chain state that are
    /// inherited from l1 [`EthTransactionValidator::apply_checks_no_state`].
    ///
    /// Applies OP-protocol specific checks that don't require chain state:
    /// - ensures tx is not eip4844
    pub fn apply_op_checks_no_state(
        &self,
        transaction: Tx,
    ) -> Result<Tx, TransactionValidationOutcome<Tx>> {
        // no support for eip4844 transactions in OP tx pool
        if transaction.is_eip4844() {
            return Err(TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TxTypeNotSupported.into(),
            ))
        }

        Ok(transaction)
    }

    /// Performs the necessary opstack specific checks on single transaction and its corresponding
    /// relevant state.
    ///
    /// Takes as parameter a transaction that has successfully passed pipeline for inherited L1
    /// checks [`EthTransactionValidator::apply_checks_against_state`].
    ///
    /// Applies OP-protocol specific checks against chain state:
    /// - ensures that the account has enough balance to cover the L1 gas cost
    pub fn apply_op_checks_against_state(
        &self,
        outcome: TransactionValidationOutcome<Tx>,
    ) -> TransactionValidationOutcome<Tx> {
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

            let encoded = valid_tx.transaction().encoded_2718();

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
                bytecode_hash,
                authorities,
            }
        }
        outcome
    }

    /// Applies Superchain validity check to single transaction.
    ///
    /// RPC queries supervisor (cross-chain oracle), which stores superchain state, about
    /// transaction's validity, given transaction is a cross-chain transaction.
    pub async fn apply_checks_against_superchain_state(
        &self,
        validation_outcome: TransactionValidationOutcome<Tx>,
    ) -> TransactionValidationOutcome<Tx> {
        let mut err = None;
        if let TransactionValidationOutcome::Valid { ref transaction, .. } = validation_outcome {
            // Interop cross tx validation
            if let Some(cross_chain_tx_res) =
                self.is_valid_cross_tx(transaction.transaction()).await
            {
                match cross_chain_tx_res {
                    Ok(()) => {
                        // valid interop tx
                        transaction.transaction().set_interop_deadline(
                            self.block_timestamp() + TRANSACTION_VALIDITY_WINDOW_SECS,
                        )
                    }
                    Err(e) => {
                        err = Some(match e {
                            InvalidCrossTx::CrossChainTxPreInterop => {
                                InvalidTransactionError::TxTypeNotSupported.into()
                            }
                            e => InvalidPoolTransactionError::Other(Box::new(e)),
                        })
                    }
                }
            } // else is not cross-chain tx
        }

        if let Some(err) = err {
            // consume valid transaction
            if let TransactionValidationOutcome::Valid { transaction, .. } = validation_outcome {
                return TransactionValidationOutcome::Invalid(transaction.into_transaction(), err)
            }
        }

        validation_outcome
    }

    /// Wrapper for [`SupervisorClient::is_valid_cross_tx`].
    pub async fn is_valid_cross_tx(&self, tx: &Tx) -> Option<Result<(), InvalidCrossTx>> {
        // We don't need to check for deposit transaction in here, because they won't come from
        // txpool
        self.supervisor_client
            .as_ref()?
            .is_valid_cross_tx(
                tx.access_list(),
                tx.hash(),
                self.block_info.timestamp.load(Ordering::Relaxed),
                Some(TRANSACTION_VALIDITY_WINDOW_SECS),
                self.fork_tracker.is_interop_activated(),
            )
            .await
    }
}

impl<Client, Tx> TransactionValidator for OpTransactionValidator<Client, Tx>
where
    Client: ChainSpecProvider<ChainSpec: OpHardforks> + StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction + OpPooledTx,
{
    type Transaction = Tx;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        self.validate_one(origin, transaction).await
    }

    async fn validate_transactions(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        self.validate_all(transactions).await
    }

    async fn validate_transactions_with_origin(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = Self::Transaction> + Send,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        self.validate_transactions(transactions.into_iter().map(|tx| (origin, tx)).collect()).await
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

/// Keeps track of whether certain forks are activated
#[derive(Debug)]
pub(crate) struct OpForkTracker {
    /// Tracks if interop is activated at the block's timestamp.
    interop: AtomicBool,
}

impl OpForkTracker {
    /// Returns `true` if Interop fork is activated.
    pub(crate) fn is_interop_activated(&self) -> bool {
        self.interop.load(Ordering::Relaxed)
    }
}
