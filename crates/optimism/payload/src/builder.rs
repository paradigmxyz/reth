//! Optimism payload builder implementation.

use crate::{
    error::OptimismPayloadBuilderError,
    payload::{OptimismBuiltPayload, OptimismPayloadBuilderAttributes},
};
use alloy_primitives::U256;
use reth_basic_payload_builder::*;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::{ChainSpec, EthereumHardforks, OptimismHardfork};
use reth_evm::{system_calls::pre_block_beacon_root_contract_call, ConfigureEvm};
use reth_execution_types::ExecutionOutcome;
use reth_payload_builder::error::PayloadBuilderError;
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_primitives::{
    constants::BEACON_NONCE, eip4844::calculate_excess_blob_gas, proofs, transaction::WithEncoded,
    Address, Block, Bytes, Header, IntoRecoveredTransaction, Receipt, SealedBlock,
    TransactionSigned, TransactionSignedEcRecovered, TxType, B256, EMPTY_OMMER_ROOT_HASH,
};
use reth_provider::{ProviderError, StateProviderFactory};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::{
    noop::NoopTransactionPool, BestTransactionsAttributes, TransactionPool,
};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use revm::{
    db::states::bundle_state::BundleRetention,
    primitives::{BlockEnv, CfgEnvWithHandlerCfg, EVMError, EnvWithHandlerCfg, ExecutionResult},
    State,
};
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Optimism's payload builder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimismPayloadBuilder<EvmConfig> {
    /// The rollup's compute pending block configuration option.
    // TODO(clabby): Implement this feature.
    compute_pending_block: bool,
    /// The type responsible for creating the evm.
    evm_config: EvmConfig,
}

impl<EvmConfig> OptimismPayloadBuilder<EvmConfig> {
    /// `OptimismPayloadBuilder` constructor.
    pub const fn new(evm_config: EvmConfig) -> Self {
        Self { compute_pending_block: true, evm_config }
    }

    /// Sets the rollup's compute pending block configuration option.
    pub const fn set_compute_pending_block(mut self, compute_pending_block: bool) -> Self {
        self.compute_pending_block = compute_pending_block;
        self
    }

    /// Enables the rollup's compute pending block configuration option.
    pub const fn compute_pending_block(self) -> Self {
        self.set_compute_pending_block(true)
    }

    /// Returns the rollup's compute pending block configuration option.
    pub const fn is_compute_pending_block(&self) -> bool {
        self.compute_pending_block
    }

    /// Initializes the pre-block state for payload building.
    ///
    /// # Arguments
    ///
    /// * `payload_config` - Payload configuration containing attributes and environments for the block
    /// * `evm_config` - EVM configuration for the execution environment
    /// * `db` - Mutable reference to the database
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the pre-block state is successfully initialized.
    /// * `Err(PayloadBuilderError::Internal(_))` if applying the beacon root contract call fails.
    /// * `Err(PayloadBuilderError::other(OptimismPayloadBuilderError::ForceCreate2DeployerFail))` if ensuring the create2 deployer fails.
    pub fn init_pre_block_state<DB>(
        &self,
        payload_config: &PayloadConfig<OptimismPayloadBuilderAttributes>,
        evm_config: EvmConfig,
        db: &mut revm::State<DB>,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: revm::Database<Error = ProviderError>,
        EvmConfig: ConfigureEvm,
    {
        let PayloadConfig {
            initialized_block_env,
            initialized_cfg,
            parent_block,
            attributes,
            chain_spec,
            ..
        } = payload_config;

        debug!(target: "payload_builder", id=%attributes.payload_attributes.payload_id(), parent_hash = ?parent_block.hash(), parent_number = parent_block.number, "building new payload");

        pre_block_beacon_root_contract_call(
            db,
            &evm_config,
            &chain_spec,
            &initialized_cfg,
            &initialized_block_env,
            attributes.payload_attributes.parent_beacon_block_root,
        )
        .map_err(|err| {
            warn!(target: "payload_builder",
                parent_hash=%parent_block.hash(),
                %err,
                "failed to apply beacon root contract call for empty payload"
            );
            PayloadBuilderError::Internal(err.into())
        })?;

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        reth_evm_optimism::ensure_create2_deployer(
            chain_spec.clone(),
            attributes.payload_attributes.timestamp,
            db,
        )
        .map_err(|err| {
            warn!(target: "payload_builder", %err, "missing create2 deployer, skipping block.");
            PayloadBuilderError::other(OptimismPayloadBuilderError::ForceCreate2DeployerFail)
        })
    }

    /// Constructs new block attributes with a populated list of transactions
    /// This method uses the provided payload attributes and transaction pool to populate the
    /// block attributes.
    ///
    /// # Arguments
    ///
    /// * `pool` - The transaction pool from which to retrieve pooled transactions
    /// * `payload_config` - Reference to the payload configuration containing attributes and environments
    /// * `db` - Mutable reference to the database
    /// * `cancel` - Marker used to signal that the payload builder should cancel building the block
    ///
    /// # Returns
    ///
    /// * `Ok(OptimismBlockAttributes<EvmConfig>)` with populated block attributes on success
    /// * `Err(PayloadBuilderError::BuildOutcomeCancelled)` if the operation was cancelled
    /// * `Err(PayloadBuilderError::BlobTransactionRejected)` if a blob transaction is encountered
    /// * `Err(PayloadBuilderError::TransactionEcRecoverFailed)` if EC recovery fails for a transaction
    /// * `Err(PayloadBuilderError::EvmExecutionError(_))` if an EVM execution error occurs
    pub fn construct_block_attributes<Pool, DB>(
        &self,
        pool: Pool,
        payload_config: &PayloadConfig<OptimismPayloadBuilderAttributes>,
        db: &mut revm::State<DB>,
        cancel: &Cancelled,
    ) -> Result<OptimismBlockAttributes<EvmConfig>, PayloadBuilderError>
    where
        Pool: TransactionPool,
        DB: revm::Database<Error = ProviderError>,
        EvmConfig: ConfigureEvm,
    {
        let mut op_block_attributes =
            OptimismBlockAttributes::new(payload_config, self.evm_config.clone());

        // add sequencer transactions to block
        op_block_attributes.add_sequencer_transactions(
            &payload_config.attributes.transactions,
            db,
            &cancel,
        )?;

        // add pooled transactions to block
        op_block_attributes.add_pooled_transactions(&pool, db, &cancel)?;

        Ok(op_block_attributes)
    }

    /// Constructs a built payload from the block attributes and execution outcomes
    ///
    /// This method assembles the final `OptimismBuiltPayload` by combining the block attributes,
    /// execution outcomes, state roots, and other necessary components. It ensures that all
    /// parts of the payload are correctly integrated and sealed for submission.
    ///
    /// # Arguments
    ///
    /// * `op_block_attributes` - The populated `OptimismBlockAttributes` containing block details
    /// * `parent_block` - Reference to the parent sealed block
    /// * `execution_outcome` - The outcome of executing transactions within the block
    /// * `state_root` - The resulting state root after transaction execution
    /// * `withdrawals_outcome` - The outcome of processing withdrawals
    /// * `hashed_state` - The hashed post-state after applying all transitions
    /// * `trie_output` - Updates to the trie based on state changes
    /// * `extra_data` - Additional data to include in the block header
    ///
    /// # Returns
    ///
    /// An `OptimismBuiltPayload` representing the fully constructed block payload.
    pub fn construct_built_payload(
        &self,
        op_block_attributes: OptimismBlockAttributes<EvmConfig>,
        parent_block: Arc<SealedBlock>,
        execution_outcome: ExecutionOutcome,
        state_root: B256,
        withdrawals_outcome: WithdrawalsOutcome,
        hashed_state: HashedPostState,
        trie_output: TrieUpdates,
        extra_data: Bytes,
    ) -> OptimismBuiltPayload
    where
        EvmConfig: ConfigureEvm,
    {
        let chain_spec = op_block_attributes.chain_spec;
        let block_number = op_block_attributes.initialized_block_env.number.to::<u64>();
        let timestamp = op_block_attributes.payload_attributes.timestamp();

        let receipts_root = execution_outcome
            .optimism_receipts_root_slow(block_number, chain_spec.as_ref(), timestamp)
            .expect("Number is in range");

        let logs_bloom =
            execution_outcome.block_logs_bloom(block_number).expect("Number is in range");

        // create the block header
        let transactions_root =
            proofs::calculate_transaction_root(&op_block_attributes.executed_txs);

        let mut excess_blob_gas = None;
        let mut blob_gas_used = None;

        // only determine cancun fields when active
        if chain_spec.is_cancun_active_at_timestamp(timestamp) {
            excess_blob_gas = if chain_spec.is_cancun_active_at_timestamp(parent_block.timestamp) {
                let parent_excess_blob_gas = parent_block.excess_blob_gas.unwrap_or_default();
                let parent_blob_gas_used = parent_block.blob_gas_used.unwrap_or_default();
                Some(calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
            } else {
                // for the first post-fork block, both parent.blob_gas_used and
                // parent.excess_blob_gas are evaluated as 0
                Some(calculate_excess_blob_gas(0, 0))
            };

            blob_gas_used = Some(0);
        }

        let header = Header {
            parent_hash: parent_block.hash(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: op_block_attributes.initialized_block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: withdrawals_outcome.withdrawals_root,
            logs_bloom,
            timestamp,
            mix_hash: op_block_attributes.payload_attributes.prev_randao(),
            nonce: BEACON_NONCE,
            base_fee_per_gas: Some(op_block_attributes.base_fee),
            number: parent_block.number + 1,
            gas_limit: op_block_attributes.block_gas_limit,
            difficulty: U256::ZERO,
            gas_used: op_block_attributes.cumulative_gas_used,
            extra_data,
            parent_beacon_block_root: op_block_attributes
                .payload_attributes
                .parent_beacon_block_root(),
            blob_gas_used,
            excess_blob_gas,
            requests_root: None,
        };

        let block = Block {
            header,
            body: op_block_attributes.executed_txs,
            ommers: vec![],
            withdrawals: withdrawals_outcome.withdrawals,
            requests: None,
        };

        let sealed_block = block.seal_slow();
        debug!(target: "payload_builder", ?sealed_block, "sealed built block");

        // create the executed block data
        let executed = ExecutedBlock {
            block: Arc::new(sealed_block.clone()),
            senders: Arc::new(op_block_attributes.executed_senders),
            execution_output: Arc::new(execution_outcome),
            hashed_state: Arc::new(hashed_state),
            trie: Arc::new(trie_output),
        };

        let payload = OptimismBuiltPayload::new(
            op_block_attributes.payload_attributes.payload_id(),
            sealed_block,
            op_block_attributes.total_fees,
            chain_spec,
            op_block_attributes.payload_attributes,
            Some(executed),
        );

        payload
    }

    /// Constructs the execution and withdrawal outcomes resulting from transactions in the block.
    ///
    /// # Arguments
    ///
    /// * `op_block_attributes` - Reference to the `OptimismBlockAttributes` containing block details
    /// * `db` - Mutable reference to the database
    ///
    /// # Returns
    ///
    /// * `Ok((WithdrawalsOutcome, ExecutionOutcome))` containing the outcomes of withdrawals and transaction executions.
    /// * `Err(ProviderError)` if processing withdrawals fails or state merging encounters an error.
    pub fn construct_outcome<DB>(
        &self,
        op_block_attributes: &OptimismBlockAttributes<EvmConfig>,
        db: &mut revm::State<DB>,
    ) -> Result<(WithdrawalsOutcome, ExecutionOutcome), ProviderError>
    where
        EvmConfig: ConfigureEvm,
        DB: revm::Database<Error = ProviderError>,
    {
        let timestamp = op_block_attributes.payload_attributes.timestamp();

        let withdrawals_outcome = commit_withdrawals(
            db,
            &op_block_attributes.chain_spec,
            timestamp,
            op_block_attributes.payload_attributes.withdrawals().to_owned(),
        )?;

        // merge all transitions into bundle state, this would apply the withdrawal balance changes
        // and 4788 contract call
        db.merge_transitions(BundleRetention::PlainState);

        let block_number = op_block_attributes.initialized_block_env.number.to::<u64>();

        let execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![op_block_attributes.receipts.to_owned()].into(),
            block_number,
            Vec::new(),
        );

        Ok((withdrawals_outcome, execution_outcome))
    }

    /// Constructs an Ethereum transaction payload from the transactions sent through the
    /// Payload attributes by the sequencer. If the `no_tx_pool` argument is passed in
    /// the payload attributes, the transaction pool will be ignored and the only transactions
    /// included in the payload will be those sent through the attributes.
    ///
    /// Given build arguments including an Ethereum client, transaction pool,
    /// and configuration, this function creates a transaction payload. Returns
    /// a result indicating success with the payload or an error in case of failure.
    fn build_payload<Pool, Client>(
        &self,
        evm_config: EvmConfig,
        args: BuildArguments<Pool, Client, OptimismPayloadBuilderAttributes, OptimismBuiltPayload>,
        _compute_pending_block: bool,
    ) -> Result<BuildOutcome<OptimismBuiltPayload>, PayloadBuilderError>
    where
        EvmConfig: ConfigureEvm,
        Client: StateProviderFactory,
        Pool: TransactionPool,
    {
        let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

        let state_provider = client.state_by_block_hash(config.parent_block.hash())?;
        let state = StateProviderDatabase::new(state_provider);
        let mut db = State::builder()
            .with_database_ref(cached_reads.as_db(state))
            .with_bundle_update()
            .build();

        self.init_pre_block_state(&config, evm_config, &mut db)?;

        let op_block_attributes = match self
            .construct_block_attributes(pool, &config, &mut db, &cancel)
        {
            Ok(outcome) => Ok(outcome),
            Err(PayloadBuilderError::BuildOutcomeCancelled) => return Ok(BuildOutcome::Cancelled),
            Err(err) => Err(err),
        }?;

        // check if we have a better block
        if !is_better_payload(best_payload.as_ref(), op_block_attributes.total_fees) {
            // can skip building the block
            return Ok(BuildOutcome::Aborted {
                fees: op_block_attributes.total_fees,
                cached_reads,
            });
        }

        let (withdrawals_outcome, execution_outcome) =
            self.construct_outcome(&op_block_attributes, &mut db)?;

        // calculate the state root
        let parent_block = config.parent_block;
        let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.state().state);
        let (state_root, trie_output) = {
            let state_provider = db.database.0.inner.borrow_mut();
            state_provider.db.state_root_with_updates(hashed_state.clone()).inspect_err(|err| {
                warn!(target: "payload_builder",
                    parent_hash=%parent_block.hash(),
                    %err,
                    "failed to calculate state root for empty payload"
                );
            })?
        };

        let payload = self.construct_built_payload(
            op_block_attributes,
            parent_block,
            execution_outcome,
            state_root,
            withdrawals_outcome,
            hashed_state,
            trie_output,
            config.extra_data,
        );

        Ok(BuildOutcome::Better { payload, cached_reads })
    }
}

/// Implementation of the [`PayloadBuilder`] trait for [`OptimismPayloadBuilder`].
impl<Pool, Client, EvmConfig> PayloadBuilder<Pool, Client> for OptimismPayloadBuilder<EvmConfig>
where
    Client: StateProviderFactory,
    Pool: TransactionPool,
    EvmConfig: ConfigureEvm,
{
    type Attributes = OptimismPayloadBuilderAttributes;
    type BuiltPayload = OptimismBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, OptimismPayloadBuilderAttributes, OptimismBuiltPayload>,
    ) -> Result<BuildOutcome<OptimismBuiltPayload>, PayloadBuilderError> {
        self.build_payload(self.evm_config.clone(), args, self.compute_pending_block)
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Pool, Client, OptimismPayloadBuilderAttributes, OptimismBuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        // we want to await the job that's already in progress because that should be returned as
        // is, there's no benefit in racing another job
        MissingPayloadBehaviour::AwaitInProgress
    }

    // NOTE: this should only be used for testing purposes because this doesn't have access to L1
    // system txs, hence on_missing_payload we return [MissingPayloadBehaviour::AwaitInProgress].
    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<OptimismBuiltPayload, PayloadBuilderError> {
        let args = BuildArguments {
            client,
            config,
            // we use defaults here because for the empty payload we don't need to execute anything
            pool: NoopTransactionPool::default(),
            cached_reads: Default::default(),
            cancel: Default::default(),
            best_payload: None,
        };

        self.build_payload(self.evm_config.clone(), args, false)?
            .into_payload()
            .ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}

/// Represents the attributes and state required to build an Optimism block
///
/// This struct holds all necessary data for constructing a block on the Optimism
/// network, including executed transactions, receipts, gas usage, and EVM-specific
/// configuration parameters
#[derive(Debug)]
pub struct OptimismBlockAttributes<EvmConfig: ConfigureEvm> {
    /// Receipts corresponding to each transaction in the block
    pub receipts: Vec<Option<Receipt>>,
    /// Signed transactions executed in the block
    pub executed_txs: Vec<TransactionSigned>,
    /// Addresses of the senders of the executed transactions
    pub executed_senders: Vec<Address>,
    /// Indicates if the Regolith hardfork is active at the block's timestamp
    pub is_regolith: bool,
    /// Gas limit for the block
    pub block_gas_limit: u64,
    /// Base fee per execution gas
    pub base_fee: u64,
    /// Total fees collected from all transactions
    pub total_fees: U256,
    /// Cumulative gas used by all transactions
    pub cumulative_gas_used: u64,
    /// Configuration parameters for the EVM environment
    pub evm_config: EvmConfig,
    /// Initialized block environment containing block-specific information
    pub initialized_block_env: BlockEnv,
    /// Initialized configuration environment with handler settings
    pub initialized_cfg: CfgEnvWithHandlerCfg,
    /// Shared reference to the chain specification
    pub chain_spec: Arc<ChainSpec>,
    /// Payload attributes sent from op-node for block construction
    pub payload_attributes: OptimismPayloadBuilderAttributes,
}

impl<EvmConfig> OptimismBlockAttributes<EvmConfig>
where
    EvmConfig: ConfigureEvm,
{
    /// Creates a new `OptimismBlockAttributes` instance.
    ///
    /// Initializes the block attributes based on the provided payload configuration
    /// and EVM configuration.
    ///
    /// # Arguments
    ///
    /// * `payload_config` - The `PayloadConfig` containing
    ///   `OptimismPayloadBuilderAttributes`, initialized block environment, configuration
    ///   environment, and chain specification.
    /// * `evm_config` - The EVM configuration implementing the `ConfigureEvm` trait,
    ///   which dictates how the EVM should behave during transaction execution.
    pub fn new(
        payload_config: &PayloadConfig<OptimismPayloadBuilderAttributes>,
        evm_config: EvmConfig,
    ) -> Self {
        let attributes = &payload_config.attributes;
        let initialized_block_env = payload_config.initialized_block_env.clone();

        let is_regolith = payload_config.chain_spec.is_fork_active_at_timestamp(
            OptimismHardfork::Regolith,
            attributes.payload_attributes.timestamp,
        );

        let block_gas_limit: u64 = attributes.gas_limit.unwrap_or_else(|| {
            initialized_block_env
                .gas_limit
                .try_into()
                .unwrap_or(payload_config.chain_spec.max_gas_limit)
        });

        let base_fee = initialized_block_env.basefee.to::<u64>();

        Self {
            executed_senders: Vec::with_capacity(attributes.transactions.len()),
            executed_txs: Vec::with_capacity(attributes.transactions.len()),
            receipts: Vec::with_capacity(attributes.transactions.len()),
            is_regolith,
            block_gas_limit,
            base_fee,
            total_fees: U256::ZERO,
            cumulative_gas_used: 0,
            evm_config,
            initialized_block_env,
            initialized_cfg: payload_config.initialized_cfg.clone(),
            chain_spec: payload_config.chain_spec.clone(),
            payload_attributes: attributes.clone(),
        }
    }

    /// Adds sequencer transactions to the block
    ///
    /// # Arguments
    ///
    /// * `transactions` - Slice of encoded, signed transactions
    /// * `db` - Mutable reference to the databsae
    /// * `cancel` - Marker used to signal that the payload builder should cancel building the block
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful addition of all valid sequencer transactions
    /// * `Err(PayloadBuilderError::BuildOutcomeCancelled)` if the operation was cancelled
    /// * `Err(PayloadBuilderError::BlobTransactionRejected)` if a blob transaction is encountered
    /// * `Err(PayloadBuilderError::TransactionEcRecoverFailed)` if EC recovery fails for a transaction
    /// * `Err(PayloadBuilderError::EvmExecutionError(EVMError::Transaction(_)))` if an EVM execution error occurs
    pub fn add_sequencer_transactions<DB>(
        &mut self,
        transactions: &[WithEncoded<TransactionSigned>],
        db: &mut revm::State<DB>,
        cancel: &Cancelled,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: revm::Database<Error = ProviderError>,
    {
        // Add sequencer transactions to the block
        for sequencer_tx in transactions {
            // Check if the job was cancelled, if so we can exit early.
            if cancel.is_cancelled() {
                return Err(PayloadBuilderError::BuildOutcomeCancelled);
            }

            // Payload attributes transactions should never contain blob transactions.
            if sequencer_tx.value().is_eip4844() {
                return Err(PayloadBuilderError::other(
                    OptimismPayloadBuilderError::BlobTransactionRejected,
                ));
            }

            // Convert the transaction to a [TransactionSignedEcRecovered]. This is
            // purely for the purposes of utilizing the `evm_config.tx_env`` function.
            // Deposit transactions do not have signatures, so if the tx is a deposit, this
            // will just pull in its `from` address.
            let sequencer_tx =
                sequencer_tx.value().clone().try_into_ecrecovered().map_err(|_| {
                    PayloadBuilderError::other(
                        OptimismPayloadBuilderError::TransactionEcRecoverFailed,
                    )
                })?;

            if let Err(err) = self.insert_sequencer_transaction(&sequencer_tx, db) {
                match err {
                    PayloadBuilderError::EvmExecutionError(EVMError::Transaction(err)) => {
                        trace!(target: "payload_builder", %err, ?sequencer_tx, "Error in sequencer transaction, skipping.");
                        continue;
                    }
                    other => {
                        return Err(other);
                    }
                }
            }
        }

        Ok(())
    }

    /// Adds transactions to the block from the transaction pool
    ///
    /// # Arguments
    ///
    /// * `pool` - Reference to the transaction pool to retrieve transactions from
    /// * `db` - Mutable reference to the database
    /// * `cancel` - Marker used to signal that the payload builder should cancel building the block
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful addition of all valid pooled transactions
    /// * `Err(PayloadBuilderError::BuildOutcomeCancelled)` if the operation was cancelled
    /// * `Err(PayloadBuilderError::EvmExecutionError(_))` if an EVM execution error occurs
    pub fn add_pooled_transactions<DB, Pool>(
        &mut self,
        pool: &Pool,
        db: &mut revm::State<DB>,
        cancel: &Cancelled,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: revm::Database<Error = ProviderError>,
        Pool: TransactionPool,
    {
        if self.payload_attributes.no_tx_pool {
            return Ok(());
        }

        let mut best_txs = pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
            self.base_fee,
            self.initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
        ));

        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if self.cumulative_gas_used + pool_tx.gas_limit() > self.block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as
                // invalid which also removes all dependent transaction from
                // the iterator before we can continue
                best_txs.mark_invalid(&pool_tx);
                continue;
            }

            // A sequencer's block should never contain blob or deposit transactions from the pool.
            if pool_tx.is_eip4844() || pool_tx.tx_type() == TxType::Deposit as u8 {
                best_txs.mark_invalid(&pool_tx);
                continue;
            }

            // Check if the job was cancelled, if so we can exit early.
            if cancel.is_cancelled() {
                return Err(PayloadBuilderError::BuildOutcomeCancelled);
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_recovered_transaction();
            self.insert_pooled_transaction(&tx, db)?;
        }

        Ok(())
    }

    /// Insert a transaction derived from payload attributes into the block, committing state to the db
    ///
    /// # Arguments
    ///
    /// * `tx` - Reference to the signed and recovered sequencer transaction
    /// * `db` - Mutable reference to the database
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful execution and insertion of the sequencer transaction
    /// * `Err(PayloadBuilderError::AccountLoadFailed(_))` if loading the depositor account fails
    /// * `Err(PayloadBuilderError::EvmExecutionError(_))` if an EVM execution error occurs
    pub fn insert_sequencer_transaction<DB>(
        &mut self,
        tx: &TransactionSignedEcRecovered,
        db: &mut revm::State<DB>,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: revm::Database<Error = ProviderError>,
    {
        // Cache the depositor account prior to the state transition for the deposit nonce.
        //
        // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
        // were not introduced in Bedrock. In addition, regular transactions don't have deposit
        // nonces, so we don't need to touch the DB for those.
        let depositor = (self.is_regolith && tx.is_deposit())
            .then(|| {
                db.load_cache_account(tx.signer()).map(|acc| acc.account_info().unwrap_or_default())
            })
            .transpose()
            .map_err(|_| {
                PayloadBuilderError::other(OptimismPayloadBuilderError::AccountLoadFailed(
                    tx.signer(),
                ))
            })?;

        let execution_result = evm_transact_commit(
            tx,
            self.initialized_cfg.clone(),
            self.initialized_block_env.clone(),
            self.evm_config.clone(),
            db,
        )
        .map_err(PayloadBuilderError::EvmExecutionError)?;

        self.cumulative_gas_used += execution_result.gas_used();

        let receipt = Receipt {
            tx_type: tx.tx_type(),
            success: execution_result.is_success(),
            cumulative_gas_used: self.cumulative_gas_used,
            logs: execution_result.into_logs().into_iter().map(Into::into).collect(),
            deposit_nonce: depositor.map(|account| account.nonce),
            deposit_receipt_version: self
                .chain_spec
                .is_fork_active_at_timestamp(
                    OptimismHardfork::Canyon,
                    self.payload_attributes.payload_attributes.timestamp,
                )
                .then_some(1),
        };

        self.receipts.push(Some(receipt));
        self.executed_senders.push(tx.signer());
        self.executed_txs.push(tx.to_owned().into_signed());

        Ok(())
    }

    /// Inserts a transaction from the transaction pool into the block, committing state to the db
    ///
    /// # Arguments
    ///
    /// * `tx` - Reference to the signed and recovered pooled transaction
    /// * `db` - Mutable reference to the database
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful execution and insertion of the pooled transaction
    /// * `Err(PayloadBuilderError::EvmExecutionError(_))` if an EVM execution error occurs
    pub fn insert_pooled_transaction<DB>(
        &mut self,
        tx: &TransactionSignedEcRecovered,
        db: &mut revm::State<DB>,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: revm::Database<Error = ProviderError>,
    {
        let execution_result = evm_transact_commit(
            tx,
            self.initialized_cfg.clone(),
            self.initialized_block_env.clone(),
            self.evm_config.clone(),
            db,
        )
        .map_err(PayloadBuilderError::EvmExecutionError)?;

        let gas_used = execution_result.gas_used();
        self.cumulative_gas_used += gas_used;

        let receipt = Receipt {
            tx_type: tx.tx_type(),
            success: execution_result.is_success(),
            cumulative_gas_used: self.cumulative_gas_used,
            logs: execution_result.into_logs().into_iter().map(Into::into).collect(),
            deposit_nonce: None,
            deposit_receipt_version: None,
        };

        // update add to total fees
        let miner_fee = tx
            .effective_tip_per_gas(Some(self.base_fee))
            .expect("fee is always valid; execution succeeded");
        self.total_fees += U256::from(miner_fee) * U256::from(gas_used);

        self.receipts.push(Some(receipt));
        self.executed_senders.push(tx.signer());
        self.executed_txs.push(tx.to_owned().into_signed());

        Ok(())
    }
}

/// Executes a transaction within the EVM and commits the state changes to the db
///
/// # Arguments
///
/// * `transaction` - Reference to the recovered transaction to execute
/// * `initialized_cfg` - Initialized configuration environment with handler settings
/// * `initialized_block_env` - Initialized block environment containing block-specific information
/// * `evm_config` - EVM configuration
/// * `db` - Mutable reference to the database
///
/// # Returns
///
/// * `Ok(ExecutionResult)` with the result of the transaction execution on success
/// * `Err(EVMError<ProviderError>)` if the transaction execution fails due to EVM-related errors
pub fn evm_transact_commit<EvmConfig, DB>(
    transaction: &TransactionSignedEcRecovered,
    initialized_cfg: CfgEnvWithHandlerCfg,
    initialized_block_env: BlockEnv,
    evm_config: EvmConfig,
    db: &mut revm::State<DB>,
) -> Result<ExecutionResult, EVMError<ProviderError>>
where
    DB: revm::Database<Error = ProviderError>,
    EvmConfig: ConfigureEvm,
{
    let env = EnvWithHandlerCfg::new_with_cfg_env(
        initialized_cfg,
        initialized_block_env,
        evm_config.tx_env(transaction),
    );
    let mut evm = evm_config.evm_with_env(db, env);

    evm.transact_commit()
}
