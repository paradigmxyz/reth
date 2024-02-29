//! Ethereum transaction validator.

use crate::{
    blobstore::BlobStore,
    error::{Eip4844PoolTransactionError, InvalidPoolTransactionError},
    traits::TransactionOrigin,
    validate::{ValidTransaction, ValidationTask, MAX_INIT_CODE_BYTE_SIZE},
    EthBlobTransactionSidecar, EthPoolTransaction, LocalTransactionConfig, PoolTransaction,
    TransactionValidationOutcome, TransactionValidationTaskExecutor, TransactionValidator,
};
use reth_primitives::{
    constants::{
        eip4844::{MAINNET_KZG_TRUSTED_SETUP, MAX_BLOBS_PER_BLOCK},
        ETHEREUM_BLOCK_GAS_LIMIT,
    },
    kzg::KzgSettings,
    revm::compat::calculate_intrinsic_gas_after_merge,
    ChainSpec, GotExpected, InvalidTransactionError, SealedBlock, EIP1559_TX_TYPE_ID,
    EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, LEGACY_TX_TYPE_ID,
};
use reth_provider::{AccountReader, BlockReaderIdExt, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::{
    marker::PhantomData,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::sync::Mutex;

#[cfg(feature = "optimism")]
use reth_revm::optimism::RethL1BlockInfo;

use super::constants::DEFAULT_MAX_TX_INPUT_BYTES;

/// Validator for Ethereum transactions.
#[derive(Debug, Clone)]
pub struct EthTransactionValidator<Client, T> {
    /// The type that performs the actual validation.
    inner: Arc<EthTransactionValidatorInner<Client, T>>,
}

impl<Client, Tx> EthTransactionValidator<Client, Tx>
where
    Client: StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction,
{
    /// Validates a single transaction.
    ///
    /// See also [TransactionValidator::validate_transaction]
    pub fn validate_one(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
    ) -> TransactionValidationOutcome<Tx> {
        self.inner.validate_one(origin, transaction)
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

impl<Client, Tx> TransactionValidator for EthTransactionValidator<Client, Tx>
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
        self.inner.on_new_head_block(new_tip_block)
    }
}

/// A [TransactionValidator] implementation that validates ethereum transaction.
#[derive(Debug)]
pub(crate) struct EthTransactionValidatorInner<Client, T> {
    /// Spec of the chain
    chain_spec: Arc<ChainSpec>,
    /// This type fetches account info from the db
    client: Client,
    /// Blobstore used for fetching re-injected blob transactions.
    blob_store: Box<dyn BlobStore>,
    /// tracks activated forks relevant for transaction validation
    fork_tracker: ForkTracker,
    /// Fork indicator whether we are using EIP-2718 type transactions.
    eip2718: bool,
    /// Fork indicator whether we are using EIP-1559 type transactions.
    eip1559: bool,
    /// Fork indicator whether we are using EIP-4844 blob transactions.
    eip4844: bool,
    /// The current max gas limit
    block_gas_limit: u64,
    /// Minimum priority fee to enforce for acceptance into the pool.
    minimum_priority_fee: Option<u128>,
    /// Stores the setup and parameters needed for validating KZG proofs.
    kzg_settings: Arc<KzgSettings>,
    /// How to handle [TransactionOrigin::Local](TransactionOrigin) transactions.
    local_transactions_config: LocalTransactionConfig,
    /// Maximum size in bytes a single transaction can have in order to be accepted into the pool.
    max_tx_input_bytes: usize,
    /// Marker for the transaction type
    _marker: PhantomData<T>,
}

// === impl EthTransactionValidatorInner ===

impl<Client, Tx> EthTransactionValidatorInner<Client, Tx> {
    /// Returns the configured chain id
    pub(crate) fn chain_id(&self) -> u64 {
        self.chain_spec.chain().id()
    }
}

impl<Client, Tx> EthTransactionValidatorInner<Client, Tx>
where
    Client: StateProviderFactory + BlockReaderIdExt,
    Tx: EthPoolTransaction,
{
    /// Validates a single transaction.
    fn validate_one(
        &self,
        origin: TransactionOrigin,
        mut transaction: Tx,
    ) -> TransactionValidationOutcome<Tx> {
        #[cfg(feature = "optimism")]
        if transaction.is_deposit() || transaction.is_eip4844() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TxTypeNotSupported.into(),
            )
        }

        // Checks for tx_type
        match transaction.tx_type() {
            LEGACY_TX_TYPE_ID => {
                // Accept legacy transactions
            }
            EIP2930_TX_TYPE_ID => {
                // Accept only legacy transactions until EIP-2718/2930 activates
                if !self.eip2718 {
                    return TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidTransactionError::Eip2930Disabled.into(),
                    )
                }
            }
            EIP1559_TX_TYPE_ID => {
                // Reject dynamic fee transactions until EIP-1559 activates.
                if !self.eip1559 {
                    return TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidTransactionError::Eip1559Disabled.into(),
                    )
                }
            }
            EIP4844_TX_TYPE_ID => {
                // Reject blob transactions.
                if !self.eip4844 {
                    return TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidTransactionError::Eip4844Disabled.into(),
                    )
                }
            }

            _ => {
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::TxTypeNotSupported.into(),
                )
            }
        };

        // Reject transactions over defined size to prevent DOS attacks
        if transaction.size() > self.max_tx_input_bytes {
            let size = transaction.size();
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::OversizedData(size, self.max_tx_input_bytes),
            )
        }

        // Check whether the init code size has been exceeded.
        if self.fork_tracker.is_shanghai_activated() {
            if let Err(err) = ensure_max_init_code_size(&transaction, MAX_INIT_CODE_BYTE_SIZE) {
                return TransactionValidationOutcome::Invalid(transaction, err)
            }
        }

        // Checks for gas limit
        if transaction.gas_limit() > self.block_gas_limit {
            let gas_limit = transaction.gas_limit();
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::ExceedsGasLimit(gas_limit, self.block_gas_limit),
            )
        }

        // Ensure max_priority_fee_per_gas (if EIP1559) is less than max_fee_per_gas if any.
        if transaction.max_priority_fee_per_gas() > Some(transaction.max_fee_per_gas()) {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TipAboveFeeCap.into(),
            )
        }

        // Drop non-local transactions with a fee lower than the configured fee for acceptance into
        // the pool.
        if !self.local_transactions_config.is_local(origin, transaction.sender()) &&
            transaction.is_eip1559() &&
            transaction.max_priority_fee_per_gas() < self.minimum_priority_fee
        {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidPoolTransactionError::Underpriced,
            )
        }

        // Checks for chainid
        if let Some(chain_id) = transaction.chain_id() {
            if chain_id != self.chain_id() {
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::ChainIdMismatch.into(),
                )
            }
        }

        // intrinsic gas checks
        let is_shanghai = self.fork_tracker.is_shanghai_activated();
        if let Err(err) = ensure_intrinsic_gas(&transaction, is_shanghai) {
            return TransactionValidationOutcome::Invalid(transaction, err)
        }

        // light blob tx pre-checks
        if transaction.is_eip4844() {
            // Cancun fork is required for blob txs
            if !self.fork_tracker.is_cancun_activated() {
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::TxTypeNotSupported.into(),
                )
            }

            let blob_count = transaction.blob_count();
            if blob_count == 0 {
                // no blobs
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidPoolTransactionError::Eip4844(
                        Eip4844PoolTransactionError::NoEip4844Blobs,
                    ),
                )
            }

            if blob_count > MAX_BLOBS_PER_BLOCK {
                // too many blobs
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidPoolTransactionError::Eip4844(
                        Eip4844PoolTransactionError::TooManyEip4844Blobs {
                            have: blob_count,
                            permitted: MAX_BLOBS_PER_BLOCK,
                        },
                    ),
                )
            }
        }

        let account = match self
            .client
            .latest()
            .and_then(|state| state.basic_account(transaction.sender()))
        {
            Ok(account) => account.unwrap_or_default(),
            Err(err) => {
                return TransactionValidationOutcome::Error(*transaction.hash(), Box::new(err))
            }
        };

        // Signer account shouldn't have bytecode. Presence of bytecode means this is a
        // smartcontract.
        if account.has_bytecode() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::SignerAccountHasBytecode.into(),
            )
        }

        // Checks for nonce
        if transaction.nonce() < account.nonce {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::NonceNotConsistent.into(),
            )
        }

        #[cfg(not(feature = "optimism"))]
        let cost = transaction.cost();

        #[cfg(feature = "optimism")]
        let cost = {
            let block = match self
                .client
                .block_by_number_or_tag(reth_primitives::BlockNumberOrTag::Latest)
            {
                Ok(Some(block)) => block,
                Ok(None) => {
                    return TransactionValidationOutcome::Error(
                        *transaction.hash(),
                        "Latest block should be found".into(),
                    )
                }
                Err(err) => {
                    return TransactionValidationOutcome::Error(*transaction.hash(), Box::new(err))
                }
            };

            let mut encoded = reth_primitives::bytes::BytesMut::default();
            transaction.to_recovered_transaction().encode_enveloped(&mut encoded);
            let cost_addition = match reth_revm::optimism::extract_l1_info(&block).map(|info| {
                info.l1_tx_data_fee(
                    &self.chain_spec,
                    block.timestamp,
                    &encoded,
                    transaction.is_deposit(),
                )
            }) {
                Ok(Ok(cost)) => cost,
                Err(err) => {
                    return TransactionValidationOutcome::Error(*transaction.hash(), Box::new(err))
                }
                _ => {
                    return TransactionValidationOutcome::Error(
                        *transaction.hash(),
                        "L1BlockInfoError".into(),
                    )
                }
            };

            transaction.cost().saturating_add(cost_addition)
        };

        // Checks for max cost
        if cost > account.balance {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::InsufficientFunds(
                    GotExpected { got: account.balance, expected: cost }.into(),
                )
                .into(),
            )
        }

        let mut maybe_blob_sidecar = None;

        // heavy blob tx validation
        if transaction.is_eip4844() {
            // extract the blob from the transaction
            match transaction.take_blob() {
                EthBlobTransactionSidecar::None => {
                    // this should not happen
                    return TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidTransactionError::TxTypeNotSupported.into(),
                    )
                }
                EthBlobTransactionSidecar::Missing => {
                    // This can happen for re-injected blob transactions (on re-org), since the blob
                    // is stripped from the transaction and not included in a block.
                    // check if the blob is in the store, if it's included we previously validated
                    // it and inserted it
                    if let Ok(true) = self.blob_store.contains(*transaction.hash()) {
                        // validated transaction is already in the store
                    } else {
                        return TransactionValidationOutcome::Invalid(
                            transaction,
                            InvalidPoolTransactionError::Eip4844(
                                Eip4844PoolTransactionError::MissingEip4844BlobSidecar,
                            ),
                        )
                    }
                }
                EthBlobTransactionSidecar::Present(blob) => {
                    if let Some(eip4844) = transaction.as_eip4844() {
                        // validate the blob
                        if let Err(err) = eip4844.validate_blob(&blob, &self.kzg_settings) {
                            return TransactionValidationOutcome::Invalid(
                                transaction,
                                InvalidPoolTransactionError::Eip4844(
                                    Eip4844PoolTransactionError::InvalidEip4844Blob(err),
                                ),
                            )
                        }
                        // store the extracted blob
                        maybe_blob_sidecar = Some(blob);
                    } else {
                        // this should not happen
                        return TransactionValidationOutcome::Invalid(
                            transaction,
                            InvalidTransactionError::TxTypeNotSupported.into(),
                        )
                    }
                }
            }
        }

        // Return the valid transaction
        TransactionValidationOutcome::Valid {
            balance: account.balance,
            state_nonce: account.nonce,
            transaction: ValidTransaction::new(transaction, maybe_blob_sidecar),
            // by this point assume all external transactions should be propagated
            propagate: match origin {
                TransactionOrigin::External => true,
                TransactionOrigin::Local => {
                    self.local_transactions_config.propagate_local_transactions
                }
                TransactionOrigin::Private => false,
            },
        }
    }

    fn on_new_head_block(&self, new_tip_block: &SealedBlock) {
        // update all forks
        if self.chain_spec.is_cancun_active_at_timestamp(new_tip_block.timestamp) {
            self.fork_tracker.cancun.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        if self.chain_spec.is_shanghai_active_at_timestamp(new_tip_block.timestamp) {
            self.fork_tracker.shanghai.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// A builder for [TransactionValidationTaskExecutor]
#[derive(Debug, Clone)]
pub struct EthTransactionValidatorBuilder {
    chain_spec: Arc<ChainSpec>,
    /// Fork indicator whether we are in the Shanghai stage.
    shanghai: bool,
    /// Fork indicator whether we are in the Cancun hardfork.
    cancun: bool,
    /// Whether using EIP-2718 type transactions is allowed
    eip2718: bool,
    /// Whether using EIP-1559 type transactions is allowed
    eip1559: bool,
    /// Whether using EIP-4844 type transactions is allowed
    eip4844: bool,
    /// The current max gas limit
    block_gas_limit: u64,
    /// Minimum priority fee to enforce for acceptance into the pool.
    minimum_priority_fee: Option<u128>,
    /// Determines how many additional tasks to spawn
    ///
    /// Default is 1
    additional_tasks: usize,

    /// Stores the setup and parameters needed for validating KZG proofs.
    kzg_settings: Arc<KzgSettings>,
    /// How to handle [TransactionOrigin::Local](TransactionOrigin) transactions.
    local_transactions_config: LocalTransactionConfig,
    /// Max size in bytes of a single transaction allowed
    max_tx_input_bytes: usize,
}

impl EthTransactionValidatorBuilder {
    /// Creates a new builder for the given [ChainSpec]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        // If cancun is enabled at genesis, enable it
        let cancun = chain_spec.is_cancun_active_at_timestamp(chain_spec.genesis_timestamp());

        Self {
            chain_spec,
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            minimum_priority_fee: None,
            additional_tasks: 1,
            kzg_settings: Arc::clone(&MAINNET_KZG_TRUSTED_SETUP),
            local_transactions_config: Default::default(),
            max_tx_input_bytes: DEFAULT_MAX_TX_INPUT_BYTES,

            // by default all transaction types are allowed
            eip2718: true,
            eip1559: true,
            eip4844: true,

            // shanghai is activated by default
            shanghai: true,

            // TODO: can hard enable by default once mainnet transitioned
            cancun,
        }
    }

    /// Disables the Cancun fork.
    pub const fn no_cancun(self) -> Self {
        self.set_cancun(false)
    }

    /// Whether to allow exemptions for local transaction exemptions.
    pub fn set_local_transactions_config(
        mut self,
        local_transactions_config: LocalTransactionConfig,
    ) -> Self {
        self.local_transactions_config = local_transactions_config;
        self
    }

    /// Set the Cancun fork.
    pub const fn set_cancun(mut self, cancun: bool) -> Self {
        self.cancun = cancun;
        self
    }

    /// Disables the Shanghai fork.
    pub const fn no_shanghai(self) -> Self {
        self.set_shanghai(false)
    }

    /// Set the Shanghai fork.
    pub const fn set_shanghai(mut self, shanghai: bool) -> Self {
        self.shanghai = shanghai;
        self
    }

    /// Disables the eip2718 support.
    pub const fn no_eip2718(self) -> Self {
        self.set_eip2718(false)
    }

    /// Set eip2718 support.
    pub const fn set_eip2718(mut self, eip2718: bool) -> Self {
        self.eip2718 = eip2718;
        self
    }

    /// Disables the eip1559 support.
    pub const fn no_eip1559(self) -> Self {
        self.set_eip1559(false)
    }

    /// Set the eip1559 support.
    pub const fn set_eip1559(mut self, eip1559: bool) -> Self {
        self.eip1559 = eip1559;
        self
    }

    /// Sets the [KzgSettings] to use for validating KZG proofs.
    pub fn kzg_settings(mut self, kzg_settings: Arc<KzgSettings>) -> Self {
        self.kzg_settings = kzg_settings;
        self
    }

    /// Sets a minimum priority fee that's enforced for acceptance into the pool.
    pub const fn with_minimum_priority_fee(mut self, minimum_priority_fee: u128) -> Self {
        self.minimum_priority_fee = Some(minimum_priority_fee);
        self
    }

    /// Sets the number of additional tasks to spawn.
    pub const fn with_additional_tasks(mut self, additional_tasks: usize) -> Self {
        self.additional_tasks = additional_tasks;
        self
    }

    /// Configures validation rules based on the head block's timestamp.
    ///
    /// For example, whether the Shanghai and Cancun hardfork is activated at launch.
    pub fn with_head_timestamp(mut self, timestamp: u64) -> Self {
        self.cancun = self.chain_spec.is_cancun_active_at_timestamp(timestamp);
        self.shanghai = self.chain_spec.is_shanghai_active_at_timestamp(timestamp);
        self
    }

    /// Sets a max size in bytes of a single transaction allowed into the pool
    pub const fn with_max_tx_input_bytes(mut self, max_tx_input_bytes: usize) -> Self {
        self.max_tx_input_bytes = max_tx_input_bytes;
        self
    }

    /// Builds a the [EthTransactionValidator] without spawning validator tasks.
    pub fn build<Client, Tx, S>(
        self,
        client: Client,
        blob_store: S,
    ) -> EthTransactionValidator<Client, Tx>
    where
        S: BlobStore,
    {
        let Self {
            chain_spec,
            shanghai,
            cancun,
            eip2718,
            eip1559,
            eip4844,
            block_gas_limit,
            minimum_priority_fee,
            kzg_settings,
            local_transactions_config,
            max_tx_input_bytes,
            ..
        } = self;

        let fork_tracker =
            ForkTracker { shanghai: AtomicBool::new(shanghai), cancun: AtomicBool::new(cancun) };

        let inner = EthTransactionValidatorInner {
            chain_spec,
            client,
            eip2718,
            eip1559,
            fork_tracker,
            eip4844,
            block_gas_limit,
            minimum_priority_fee,
            blob_store: Box::new(blob_store),
            kzg_settings,
            local_transactions_config,
            max_tx_input_bytes,
            _marker: Default::default(),
        };

        EthTransactionValidator { inner: Arc::new(inner) }
    }

    /// Builds a the [EthTransactionValidator] and spawns validation tasks via the
    /// [TransactionValidationTaskExecutor]
    ///
    /// The validator will spawn `additional_tasks` additional tasks for validation.
    ///
    /// By default this will spawn 1 additional task.
    pub fn build_with_tasks<Client, Tx, T, S>(
        self,
        client: Client,
        tasks: T,
        blob_store: S,
    ) -> TransactionValidationTaskExecutor<EthTransactionValidator<Client, Tx>>
    where
        T: TaskSpawner,
        S: BlobStore,
    {
        let additional_tasks = self.additional_tasks;
        let validator = self.build(client, blob_store);

        let (tx, task) = ValidationTask::new();

        // Spawn validation tasks, they are blocking because they perform db lookups
        for _ in 0..additional_tasks {
            let task = task.clone();
            tasks.spawn_blocking(Box::pin(async move {
                task.run().await;
            }));
        }

        // we spawn them on critical tasks because validation, especially for EIP-4844 can be quite
        // heavy
        tasks.spawn_critical_blocking(
            "transaction-validation-service",
            Box::pin(async move {
                task.run().await;
            }),
        );

        let to_validation_task = Arc::new(Mutex::new(tx));

        TransactionValidationTaskExecutor { validator, to_validation_task }
    }
}

/// Keeps track of whether certain forks are activated
#[derive(Debug)]
pub(crate) struct ForkTracker {
    /// Tracks if shanghai is activated at the block's timestamp.
    pub(crate) shanghai: AtomicBool,
    /// Tracks if cancun is activated at the block's timestamp.
    pub(crate) cancun: AtomicBool,
}

impl ForkTracker {
    /// Returns true if the Shanghai fork is activated.
    pub(crate) fn is_shanghai_activated(&self) -> bool {
        self.shanghai.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns true if the Shanghai fork is activated.
    pub(crate) fn is_cancun_activated(&self) -> bool {
        self.cancun.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Ensure that the code size is not greater than `max_init_code_size`.
/// `max_init_code_size` should be configurable so this will take it as an argument.
pub fn ensure_max_init_code_size<T: PoolTransaction>(
    transaction: &T,
    max_init_code_size: usize,
) -> Result<(), InvalidPoolTransactionError> {
    if transaction.kind().is_create() && transaction.input().len() > max_init_code_size {
        Err(InvalidPoolTransactionError::ExceedsMaxInitCodeSize(
            transaction.size(),
            max_init_code_size,
        ))
    } else {
        Ok(())
    }
}

/// Ensures that gas limit of the transaction exceeds the intrinsic gas of the transaction.
///
/// See also [calculate_intrinsic_gas_after_merge]
pub fn ensure_intrinsic_gas<T: PoolTransaction>(
    transaction: &T,
    is_shanghai: bool,
) -> Result<(), InvalidPoolTransactionError> {
    let access_list = transaction.access_list().map(|list| list.flattened()).unwrap_or_default();
    if transaction.gas_limit() <
        calculate_intrinsic_gas_after_merge(
            transaction.input(),
            transaction.kind(),
            &access_list,
            is_shanghai,
        )
    {
        Err(InvalidPoolTransactionError::IntrinsicGasTooLow)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // <https://github.com/paradigmxyz/reth/issues/5178>
    #[cfg(not(feature = "optimism"))]
    #[tokio::test]
    async fn validate_transaction() {
        use super::*;
        use crate::{
            blobstore::InMemoryBlobStore, CoinbaseTipOrdering, EthPooledTransaction, Pool,
            TransactionPool,
        };
        use reth_primitives::{
            hex, FromRecoveredPooledTransaction, PooledTransactionsElement, MAINNET, U256,
        };
        use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

        let raw = "0x02f914950181ad84b2d05e0085117553845b830f7df88080b9143a6040608081523462000414576200133a803803806200001e8162000419565b9283398101608082820312620004145781516001600160401b03908181116200041457826200004f9185016200043f565b92602092838201519083821162000414576200006d9183016200043f565b8186015190946001600160a01b03821692909183900362000414576060015190805193808511620003145760038054956001938488811c9816801562000409575b89891014620003f3578190601f988981116200039d575b50899089831160011462000336576000926200032a575b505060001982841b1c191690841b1781555b8751918211620003145760049788548481811c9116801562000309575b89821014620002f457878111620002a9575b5087908784116001146200023e5793839491849260009562000232575b50501b92600019911b1c19161785555b6005556007805460ff60a01b19169055600880546001600160a01b0319169190911790553015620001f3575060025469d3c21bcecceda100000092838201809211620001de57506000917fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef9160025530835282815284832084815401905584519384523093a351610e889081620004b28239f35b601190634e487b7160e01b6000525260246000fd5b90606493519262461bcd60e51b845283015260248201527f45524332303a206d696e7420746f20746865207a65726f2061646472657373006044820152fd5b0151935038806200013a565b9190601f198416928a600052848a6000209460005b8c8983831062000291575050501062000276575b50505050811b0185556200014a565b01519060f884600019921b161c191690553880808062000267565b86860151895590970196948501948893500162000253565b89600052886000208880860160051c8201928b8710620002ea575b0160051c019085905b828110620002dd5750506200011d565b60008155018590620002cd565b92508192620002c4565b60228a634e487b7160e01b6000525260246000fd5b90607f16906200010b565b634e487b7160e01b600052604160045260246000fd5b015190503880620000dc565b90869350601f19831691856000528b6000209260005b8d8282106200038657505084116200036d575b505050811b018155620000ee565b015160001983861b60f8161c191690553880806200035f565b8385015186558a979095019493840193016200034c565b90915083600052896000208980850160051c8201928c8610620003e9575b918891869594930160051c01915b828110620003d9575050620000c5565b60008155859450889101620003c9565b92508192620003bb565b634e487b7160e01b600052602260045260246000fd5b97607f1697620000ae565b600080fd5b6040519190601f01601f191682016001600160401b038111838210176200031457604052565b919080601f84011215620004145782516001600160401b038111620003145760209062000475601f8201601f1916830162000419565b92818452828287010111620004145760005b8181106200049d57508260009394955001015290565b85810183015184820184015282016200048756fe608060408181526004918236101561001657600080fd5b600092833560e01c91826306fdde0314610a1c57508163095ea7b3146109f257816318160ddd146109d35781631b4c84d2146109ac57816323b872dd14610833578163313ce5671461081757816339509351146107c357816370a082311461078c578163715018a6146107685781638124f7ac146107495781638da5cb5b1461072057816395d89b411461061d578163a457c2d714610575578163a9059cbb146104e4578163c9567bf914610120575063dd62ed3e146100d557600080fd5b3461011c578060031936011261011c57806020926100f1610b5a565b6100f9610b75565b6001600160a01b0391821683526001865283832091168252845220549051908152f35b5080fd5b905082600319360112610338576008546001600160a01b039190821633036104975760079283549160ff8360a01c1661045557737a250d5630b4cf539739df2c5dacb4c659f2488d92836bffffffffffffffffffffffff60a01b8092161786553087526020938785528388205430156104065730895260018652848920828a52865280858a205584519081527f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925863092a38554835163c45a015560e01b815290861685828581845afa9182156103dd57849187918b946103e7575b5086516315ab88c960e31b815292839182905afa9081156103dd576044879289928c916103c0575b508b83895196879586946364e329cb60e11b8652308c870152166024850152165af19081156103b6579086918991610389575b50169060065416176006558385541660604730895288865260c4858a20548860085416928751958694859363f305d71960e01b8552308a86015260248501528d60448501528d606485015260848401524260a48401525af1801561037f579084929161034c575b50604485600654169587541691888551978894859363095ea7b360e01b855284015260001960248401525af1908115610343575061030c575b5050805460ff60a01b1916600160a01b17905580f35b81813d831161033c575b6103208183610b8b565b8101031261033857518015150361011c5738806102f6565b8280fd5b503d610316565b513d86823e3d90fd5b6060809293503d8111610378575b6103648183610b8b565b81010312610374578290386102bd565b8580fd5b503d61035a565b83513d89823e3d90fd5b6103a99150863d88116103af575b6103a18183610b8b565b810190610e33565b38610256565b503d610397565b84513d8a823e3d90fd5b6103d79150843d86116103af576103a18183610b8b565b38610223565b85513d8b823e3d90fd5b6103ff919450823d84116103af576103a18183610b8b565b92386101fb565b845162461bcd60e51b81528085018790526024808201527f45524332303a20617070726f76652066726f6d20746865207a65726f206164646044820152637265737360e01b6064820152608490fd5b6020606492519162461bcd60e51b8352820152601760248201527f74726164696e6720697320616c7265616479206f70656e0000000000000000006044820152fd5b608490602084519162461bcd60e51b8352820152602160248201527f4f6e6c79206f776e65722063616e2063616c6c20746869732066756e6374696f6044820152603760f91b6064820152fd5b9050346103385781600319360112610338576104fe610b5a565b9060243593303303610520575b602084610519878633610bc3565b5160018152f35b600594919454808302908382041483151715610562576127109004820391821161054f5750925080602061050b565b634e487b7160e01b815260118552602490fd5b634e487b7160e01b825260118652602482fd5b9050823461061a578260031936011261061a57610590610b5a565b918360243592338152600160205281812060018060a01b03861682526020522054908282106105c9576020856105198585038733610d31565b608490602086519162461bcd60e51b8352820152602560248201527f45524332303a2064656372656173656420616c6c6f77616e63652062656c6f77604482015264207a65726f60d81b6064820152fd5b80fd5b83833461011c578160031936011261011c57805191809380549160019083821c92828516948515610716575b6020958686108114610703578589529081156106df5750600114610687575b6106838787610679828c0383610b8b565b5191829182610b11565b0390f35b81529295507f8a35acfbc15ff81a39ae7d344fd709f28e8600b4aa8c65c6b64bfe7fe36bd19b5b8284106106cc57505050826106839461067992820101948680610668565b80548685018801529286019281016106ae565b60ff19168887015250505050151560051b8301019250610679826106838680610668565b634e487b7160e01b845260228352602484fd5b93607f1693610649565b50503461011c578160031936011261011c5760085490516001600160a01b039091168152602090f35b50503461011c578160031936011261011c576020906005549051908152f35b833461061a578060031936011261061a57600880546001600160a01b031916905580f35b50503461011c57602036600319011261011c5760209181906001600160a01b036107b4610b5a565b16815280845220549051908152f35b82843461061a578160031936011261061a576107dd610b5a565b338252600160209081528383206001600160a01b038316845290528282205460243581019290831061054f57602084610519858533610d31565b50503461011c578160031936011261011c576020905160128152f35b83833461011c57606036600319011261011c5761084e610b5a565b610856610b75565b6044359160018060a01b0381169485815260209560018752858220338352875285822054976000198903610893575b505050906105199291610bc3565b85891061096957811561091a5733156108cc5750948481979861051997845260018a528284203385528a52039120558594938780610885565b865162461bcd60e51b8152908101889052602260248201527f45524332303a20617070726f766520746f20746865207a65726f206164647265604482015261737360f01b6064820152608490fd5b865162461bcd60e51b81529081018890526024808201527f45524332303a20617070726f76652066726f6d20746865207a65726f206164646044820152637265737360e01b6064820152608490fd5b865162461bcd60e51b8152908101889052601d60248201527f45524332303a20696e73756666696369656e7420616c6c6f77616e63650000006044820152606490fd5b50503461011c578160031936011261011c5760209060ff60075460a01c1690519015158152f35b50503461011c578160031936011261011c576020906002549051908152f35b50503461011c578060031936011261011c57602090610519610a12610b5a565b6024359033610d31565b92915034610b0d5783600319360112610b0d57600354600181811c9186908281168015610b03575b6020958686108214610af05750848852908115610ace5750600114610a75575b6106838686610679828b0383610b8b565b929550600383527fc2575a0e9e593c00f959f8c92f12db2869c3395a3b0502d05e2516446f71f85b5b828410610abb575050508261068394610679928201019438610a64565b8054868501880152928601928101610a9e565b60ff191687860152505050151560051b83010192506106798261068338610a64565b634e487b7160e01b845260229052602483fd5b93607f1693610a44565b8380fd5b6020808252825181830181905290939260005b828110610b4657505060409293506000838284010152601f8019910116010190565b818101860151848201604001528501610b24565b600435906001600160a01b0382168203610b7057565b600080fd5b602435906001600160a01b0382168203610b7057565b90601f8019910116810190811067ffffffffffffffff821117610bad57604052565b634e487b7160e01b600052604160045260246000fd5b6001600160a01b03908116918215610cde5716918215610c8d57600082815280602052604081205491808310610c3957604082827fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef958760209652828652038282205586815220818154019055604051908152a3565b60405162461bcd60e51b815260206004820152602660248201527f45524332303a207472616e7366657220616d6f756e7420657863656564732062604482015265616c616e636560d01b6064820152608490fd5b60405162461bcd60e51b815260206004820152602360248201527f45524332303a207472616e7366657220746f20746865207a65726f206164647260448201526265737360e81b6064820152608490fd5b60405162461bcd60e51b815260206004820152602560248201527f45524332303a207472616e736665722066726f6d20746865207a65726f206164604482015264647265737360d81b6064820152608490fd5b6001600160a01b03908116918215610de25716918215610d925760207f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925918360005260018252604060002085600052825280604060002055604051908152a3565b60405162461bcd60e51b815260206004820152602260248201527f45524332303a20617070726f766520746f20746865207a65726f206164647265604482015261737360f01b6064820152608490fd5b60405162461bcd60e51b8152602060048201526024808201527f45524332303a20617070726f76652066726f6d20746865207a65726f206164646044820152637265737360e01b6064820152608490fd5b90816020910312610b7057516001600160a01b0381168103610b70579056fea2646970667358221220285c200b3978b10818ff576bb83f2dc4a2a7c98dfb6a36ea01170de792aa652764736f6c63430008140033000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000c0000000000000000000000000d3fd4f95820a9aa848ce716d6c200eaefb9a2e4900000000000000000000000000000000000000000000000000000000000000640000000000000000000000000000000000000000000000000000000000000003543131000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000035431310000000000000000000000000000000000000000000000000000000000c001a04e551c75810ffdfe6caff57da9f5a8732449f42f0f4c57f935b05250a76db3b6a046cd47e6d01914270c1ec0d9ac7fae7dfb240ec9a8b6ec7898c4d6aa174388f2";

        let data = hex::decode(raw).unwrap();
        let tx = PooledTransactionsElement::decode_enveloped(data.into()).unwrap();

        let transaction = EthPooledTransaction::from_recovered_pooled_transaction(
            tx.try_into_ecrecovered().unwrap(),
        );
        let res = ensure_intrinsic_gas(&transaction, false);
        assert!(res.is_ok());
        let res = ensure_intrinsic_gas(&transaction, true);
        assert!(res.is_ok());

        let provider = MockEthProvider::default();
        provider.add_account(
            transaction.sender(),
            ExtendedAccount::new(transaction.nonce(), U256::MAX),
        );
        let blob_store = InMemoryBlobStore::default();
        let validator = EthTransactionValidatorBuilder::new(MAINNET.clone())
            .build(provider, blob_store.clone());

        let outcome = validator.validate_one(TransactionOrigin::External, transaction.clone());

        assert!(outcome.is_valid());

        let pool =
            Pool::new(validator, CoinbaseTipOrdering::default(), blob_store, Default::default());

        let res = pool.add_external_transaction(transaction.clone()).await;
        assert!(res.is_ok());
        let tx = pool.get(transaction.hash());
        assert!(tx.is_some());
    }

    #[cfg(feature = "optimism")]
    #[tokio::test(flavor = "multi_thread")]
    async fn validate_optimism_transaction() {
        use crate::{blobstore::InMemoryBlobStore, traits::EthPooledTransaction};
        use reth_primitives::{
            Signature, Transaction, TransactionKind, TransactionSigned,
            TransactionSignedEcRecovered, TxDeposit, MAINNET, U256,
        };
        use reth_provider::test_utils::MockEthProvider;
        use reth_tasks::TokioTaskExecutor;

        let client = MockEthProvider::default();
        let validator =
            // EthTransactionValidator::new(client, MAINNET.clone(), TokioTaskExecutor::default());
            crate::validate::EthTransactionValidatorBuilder::new(MAINNET.clone()).no_shanghai().no_cancun().build_with_tasks(client, TokioTaskExecutor::default(), InMemoryBlobStore::default());
        let origin = crate::TransactionOrigin::External;
        let signer = Default::default();
        let deposit_tx = Transaction::Deposit(TxDeposit {
            source_hash: Default::default(),
            from: signer,
            to: TransactionKind::Create,
            mint: None,
            value: reth_primitives::TxValue::from(U256::ZERO),
            gas_limit: 0u64,
            is_system_transaction: false,
            input: Default::default(),
        });
        let signature = Signature { r: U256::ZERO, s: U256::ZERO, odd_y_parity: false };
        let signed_tx = TransactionSigned::from_transaction_and_signature(deposit_tx, signature);
        let signed_recovered =
            TransactionSignedEcRecovered::from_signed_transaction(signed_tx, signer);
        let len = signed_recovered.length_without_header();
        let pooled_tx = EthPooledTransaction::new(signed_recovered, len);
        let outcome =
            crate::TransactionValidator::validate_transaction(&validator, origin, pooled_tx).await;

        let err = match outcome {
            crate::TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }
}
