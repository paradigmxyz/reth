//! Optimism payload builder implementation.

use std::{fmt::Display, sync::Arc};

use alloy_consensus::EMPTY_OMMER_ROOT_HASH;
use alloy_eips::merge::BEACON_NONCE;
use alloy_primitives::{Address, Bytes, B64, U256};
use alloy_rpc_types_engine::PayloadId;
use reth_basic_payload_builder::*;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpecProvider;
use reth_evm::{system_calls::SystemCaller, ConfigureEvm, NextBlockEnvAttributes};
use reth_execution_types::ExecutionOutcome;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::calculate_receipt_root_no_memo_optimism;
use reth_optimism_forks::OptimismHardforks;
use reth_payload_primitives::{PayloadBuilderAttributes, PayloadBuilderError};
use reth_primitives::{
    proofs,
    revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg},
    Block, BlockBody, Header, Receipt, SealedHeader, TransactionSigned, TxType,
};
use reth_provider::{ProviderError, StateProviderFactory, StateRootProvider};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::{
    noop::NoopTransactionPool, BestTransactions, BestTransactionsAttributes, BestTransactionsFor,
    TransactionPool,
};
use reth_trie::HashedPostState;
use revm::{
    db::{states::bundle_state::BundleRetention, State},
    primitives::{EVMError, EnvWithHandlerCfg, InvalidTransaction, ResultAndState},
    Database, DatabaseCommit,
};
use tracing::{debug, trace, warn};

use crate::{
    error::OptimismPayloadBuilderError,
    payload::{OpBuiltPayload, OpPayloadBuilderAttributes},
};
use op_alloy_consensus::DepositTransaction;

/// Optimism's payload builder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpPayloadBuilder<EvmConfig> {
    /// The rollup's compute pending block configuration option.
    // TODO(clabby): Implement this feature.
    pub compute_pending_block: bool,
    /// The type responsible for creating the evm.
    pub evm_config: EvmConfig,
}

impl<EvmConfig> OpPayloadBuilder<EvmConfig> {
    /// `OpPayloadBuilder` constructor.
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
}
impl<EvmConfig> OpPayloadBuilder<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    /// Returns the configured [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the targeted payload
    /// (that has the `parent` as its parent).
    pub fn cfg_and_block_env(
        &self,
        config: &PayloadConfig<OpPayloadBuilderAttributes>,
        parent: &Header,
    ) -> Result<(CfgEnvWithHandlerCfg, BlockEnv), EvmConfig::Error> {
        let next_attributes = NextBlockEnvAttributes {
            timestamp: config.attributes.timestamp(),
            suggested_fee_recipient: config.attributes.suggested_fee_recipient(),
            prev_randao: config.attributes.prev_randao(),
        };
        self.evm_config.next_cfg_and_block_env(parent, next_attributes)
    }

    /// Constructs an Optimism payload from the transactions sent via the
    /// Payload attributes by the sequencer. If the `no_tx_pool` argument is passed in
    /// the payload attributes, the transaction pool will be ignored and the only transactions
    /// included in the payload will be those sent through the attributes.
    ///
    /// Given build arguments including an Optimism client, transaction pool,
    /// and configuration, this function creates a transaction payload. Returns
    /// a result indicating success with the payload or an error in case of failure.
    fn build_payload<Client, Pool>(
        &self,
        args: BuildArguments<Pool, Client, OpPayloadBuilderAttributes, OpBuiltPayload>,
    ) -> Result<BuildOutcome<OpBuiltPayload>, PayloadBuilderError>
    where
        Client: StateProviderFactory + ChainSpecProvider<ChainSpec = OpChainSpec>,
        Pool: TransactionPool,
    {
        let (initialized_cfg, initialized_block_env) = self
            .cfg_and_block_env(&args.config, &args.config.parent_header)
            .map_err(PayloadBuilderError::other)?;

        let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

        let ctx = OpPayloadBuilderCtx {
            evm_config: self.evm_config.clone(),
            chain_spec: client.chain_spec(),
            config,
            initialized_cfg,
            initialized_block_env,
            cancel,
            best_payload,
        };

        let builder = OpBuilder {
            pool,
            // TODO(mattsse): make this configurable in the `OpPayloadBuilder` directly via an
            // additional generic
            best: best_txs::<Pool>,
        };

        let state_provider = client.state_by_block_hash(ctx.parent().hash())?;
        let state = StateProviderDatabase::new(state_provider);

        if ctx.attributes().no_tx_pool {
            let db = State::builder().with_database(state).with_bundle_update().build();
            builder.build(db, ctx)
        } else {
            // sequencer mode we can reuse cachedreads from previous runs
            let db = State::builder()
                .with_database(cached_reads.as_db_mut(state))
                .with_bundle_update()
                .build();
            builder.build(db, ctx)
        }
        .map(|out| out.with_cached_reads(cached_reads))
    }
}

/// Implementation of the [`PayloadBuilder`] trait for [`OpPayloadBuilder`].
impl<Pool, Client, EvmConfig> PayloadBuilder<Pool, Client> for OpPayloadBuilder<EvmConfig>
where
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = OpChainSpec>,
    Pool: TransactionPool,
    EvmConfig: ConfigureEvm<Header = Header>,
{
    type Attributes = OpPayloadBuilderAttributes;
    type BuiltPayload = OpBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, OpPayloadBuilderAttributes, OpBuiltPayload>,
    ) -> Result<BuildOutcome<OpBuiltPayload>, PayloadBuilderError> {
        self.build_payload(args)
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Pool, Client, OpPayloadBuilderAttributes, OpBuiltPayload>,
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
    ) -> Result<OpBuiltPayload, PayloadBuilderError> {
        let args = BuildArguments {
            client,
            config,
            // we use defaults here because for the empty payload we don't need to execute anything
            pool: NoopTransactionPool::default(),
            cached_reads: Default::default(),
            cancel: Default::default(),
            best_payload: None,
        };
        self.build_payload(args)?.into_payload().ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}

/// The type that builds the payload.
///
/// Payload building for optimism is composed of several steps.
/// The first steps are mandatory and defined by the protocol.
///
/// 1. first all System calls are applied.
/// 2. After canyon the forced deployed `create2deployer` must be loaded
/// 3. all sequencer transactions are executed (part of the payload attributes)
///
/// Depending on whether the node acts as a sequencer and is allowed to include additional
/// transactions (`no_tx_pool == false`):
/// 4. include additional transactions
///
/// And finally
/// 5. build the block: compute all roots (txs, state)
#[derive(Debug)]
pub struct OpBuilder<Pool, Best> {
    /// The transaction pool
    pool: Pool,
    /// Yields the best transaction to include if transactions from the mempool are allowed.
    // TODO(mattsse): convert this to a trait
    best: Best,
}

impl<Pool, Best> OpBuilder<Pool, Best>
where
    Pool: TransactionPool,
    Best: FnOnce(Pool, BestTransactionsAttributes) -> BestTransactionsFor<Pool>,
{
    /// Builds the payload on top of the state.
    pub fn build<EvmConfig, DB, P>(
        self,
        mut db: State<DB>,
        ctx: OpPayloadBuilderCtx<EvmConfig>,
    ) -> Result<BuildOutcomeKind<OpBuiltPayload>, PayloadBuilderError>
    where
        EvmConfig: ConfigureEvm<Header = Header>,
        DB: Database<Error = ProviderError> + AsRef<P>,
        P: StateRootProvider,
    {
        let Self { pool, best } = self;
        debug!(target: "payload_builder", id=%ctx.payload_id(), parent_header = ?ctx.parent().hash(), parent_number = ctx.parent().number, "building new payload");

        // 1. apply eip-4788 pre block contract call
        ctx.apply_pre_beacon_root_contract_call(&mut db)?;

        // 2. ensure create2deployer is force deployed
        ctx.ensure_create2_deployer(&mut db)?;

        // 3. execute sequencer transactions
        let mut info = ctx.execute_sequencer_transactions(&mut db)?;

        // 4. if mem pool transactions are requested we execute them
        if !ctx.attributes().no_tx_pool {
            let best_txs = best(pool, ctx.best_transaction_attributes());
            if let Some(cancelled) =
                ctx.execute_best_transactions::<_, Pool>(&mut info, &mut db, best_txs)?
            {
                return Ok(cancelled)
            }

            // check if the new payload is even more valuable
            if !ctx.is_better_payload(info.total_fees) {
                // can skip building the block
                return Ok(BuildOutcomeKind::Aborted { fees: info.total_fees })
            }
        }

        let WithdrawalsOutcome { withdrawals_root, withdrawals } =
            ctx.commit_withdrawals(&mut db)?;

        // merge all transitions into bundle state, this would apply the withdrawal balance changes
        // and 4788 contract call
        db.merge_transitions(BundleRetention::Reverts);

        let block_number = ctx.block_number();
        let execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![info.receipts.clone()].into(),
            block_number,
            Vec::new(),
        );
        let receipts_root = execution_outcome
            .generic_receipts_root_slow(block_number, |receipts| {
                calculate_receipt_root_no_memo_optimism(
                    receipts,
                    &ctx.chain_spec,
                    ctx.attributes().timestamp(),
                )
            })
            .expect("Number is in range");
        let logs_bloom =
            execution_outcome.block_logs_bloom(block_number).expect("Number is in range");

        // // calculate the state root
        let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.state().state);
        let (state_root, trie_output) = {
            db.database.as_ref().state_root_with_updates(hashed_state.clone()).inspect_err(
                |err| {
                    warn!(target: "payload_builder",
                    parent_header=%ctx.parent().hash(),
                        %err,
                        "failed to calculate state root for payload"
                    );
                },
            )?
        };

        // create the block header
        let transactions_root = proofs::calculate_transaction_root(&info.executed_transactions);

        // OP doesn't support blobs/EIP-4844.
        // https://specs.optimism.io/protocol/exec-engine.html#ecotone-disable-blob-transactions
        // Need [Some] or [None] based on hardfork to match block hash.
        let (excess_blob_gas, blob_gas_used) = ctx.blob_fields();
        let extra_data = ctx.extra_data()?;

        let header = Header {
            parent_hash: ctx.parent().hash(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: ctx.initialized_block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
            logs_bloom,
            timestamp: ctx.attributes().payload_attributes.timestamp,
            mix_hash: ctx.attributes().payload_attributes.prev_randao,
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(ctx.base_fee()),
            number: ctx.parent().number + 1,
            gas_limit: ctx.block_gas_limit(),
            difficulty: U256::ZERO,
            gas_used: info.cumulative_gas_used,
            extra_data,
            parent_beacon_block_root: ctx.attributes().payload_attributes.parent_beacon_block_root,
            blob_gas_used,
            excess_blob_gas,
            requests_hash: None,
        };

        // seal the block
        let block = Block {
            header,
            body: BlockBody {
                transactions: info.executed_transactions,
                ommers: vec![],
                withdrawals,
            },
        };

        let sealed_block = block.seal_slow();
        debug!(target: "payload_builder", ?sealed_block, "sealed built block");

        // create the executed block data
        let executed = ExecutedBlock {
            block: Arc::new(sealed_block.clone()),
            senders: Arc::new(info.executed_senders),
            execution_output: Arc::new(execution_outcome),
            hashed_state: Arc::new(hashed_state),
            trie: Arc::new(trie_output),
        };

        let no_tx_pool = ctx.attributes().no_tx_pool;

        let payload = OpBuiltPayload::new(
            ctx.payload_id(),
            sealed_block,
            info.total_fees,
            ctx.chain_spec.clone(),
            ctx.config.attributes,
            Some(executed),
        );

        if no_tx_pool {
            // if `no_tx_pool` is set only transactions from the payload attributes will be included
            // in the payload. In other words, the payload is deterministic and we can
            // freeze it once we've successfully built it.
            Ok(BuildOutcomeKind::Freeze(payload))
        } else {
            Ok(BuildOutcomeKind::Better { payload })
        }
    }
}

fn best_txs<Pool: TransactionPool>(
    pool: Pool,
    attr: BestTransactionsAttributes,
) -> BestTransactionsFor<Pool> {
    pool.best_transactions_with_attributes(attr)
}

/// This acts as the container for executed transactions and its byproducts (receipts, gas used)
#[derive(Default, Debug)]
pub struct ExecutionInfo {
    /// All executed transactions (unrecovered).
    pub executed_transactions: Vec<TransactionSigned>,
    /// The recovered senders for the executed transactions.
    pub executed_senders: Vec<Address>,
    /// The transaction receipts
    pub receipts: Vec<Option<Receipt>>,
    /// All gas used so far
    pub cumulative_gas_used: u64,
    /// Tracks fees from executed mempool transactions
    pub total_fees: U256,
}

impl ExecutionInfo {
    /// Create a new instance with allocated slots.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            executed_transactions: Vec::with_capacity(capacity),
            executed_senders: Vec::with_capacity(capacity),
            receipts: Vec::with_capacity(capacity),
            cumulative_gas_used: 0,
            total_fees: U256::ZERO,
        }
    }
}

/// Container type that holds all necessities to build a new payload.
#[derive(Debug)]
pub struct OpPayloadBuilderCtx<EvmConfig> {
    /// The type that knows how to perform system calls and configure the evm.
    pub evm_config: EvmConfig,
    /// The chainspec
    pub chain_spec: Arc<OpChainSpec>,
    /// How to build the payload.
    pub config: PayloadConfig<OpPayloadBuilderAttributes>,
    /// Evm Settings
    pub initialized_cfg: CfgEnvWithHandlerCfg,
    /// Block config
    pub initialized_block_env: BlockEnv,
    /// Marker to check whether the job has been cancelled.
    pub cancel: Cancelled,
    /// The currently best payload.
    pub best_payload: Option<OpBuiltPayload>,
}

impl<EvmConfig> OpPayloadBuilderCtx<EvmConfig> {
    /// Returns the parent block the payload will be build on.
    pub fn parent(&self) -> &SealedHeader {
        &self.config.parent_header
    }

    /// Returns the builder attributes.
    pub const fn attributes(&self) -> &OpPayloadBuilderAttributes {
        &self.config.attributes
    }

    /// Returns the block gas limit to target.
    pub fn block_gas_limit(&self) -> u64 {
        self.attributes()
            .gas_limit
            .unwrap_or_else(|| self.initialized_block_env.gas_limit.saturating_to())
    }

    /// Returns the block number for the block.
    pub fn block_number(&self) -> u64 {
        self.initialized_block_env.number.to()
    }

    /// Returns the current base fee
    pub fn base_fee(&self) -> u64 {
        self.initialized_block_env.basefee.to()
    }

    /// Returns the current blob gas price.
    pub fn get_blob_gasprice(&self) -> Option<u64> {
        self.initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64)
    }

    /// Returns the blob fields for the header.
    ///
    /// This will always return `Some(0)` after ecotone.
    pub fn blob_fields(&self) -> (Option<u64>, Option<u64>) {
        // OP doesn't support blobs/EIP-4844.
        // https://specs.optimism.io/protocol/exec-engine.html#ecotone-disable-blob-transactions
        // Need [Some] or [None] based on hardfork to match block hash.
        if self.is_ecotone_active() {
            (Some(0), Some(0))
        } else {
            (None, None)
        }
    }

    /// Returns the extra data for the block.
    ///
    /// After holocene this extracts the extradata from the paylpad
    pub fn extra_data(&self) -> Result<Bytes, PayloadBuilderError> {
        if self.is_holocene_active() {
            self.attributes()
                .get_holocene_extra_data(
                    self.chain_spec.base_fee_params_at_timestamp(
                        self.attributes().payload_attributes.timestamp,
                    ),
                )
                .map_err(PayloadBuilderError::other)
        } else {
            Ok(self.config.extra_data.clone())
        }
    }

    /// Returns the current fee settings for transactions from the mempool
    pub fn best_transaction_attributes(&self) -> BestTransactionsAttributes {
        BestTransactionsAttributes::new(self.base_fee(), self.get_blob_gasprice())
    }

    /// Returns the unique id for this payload job.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes().payload_id()
    }

    /// Returns true if regolith is active for the payload.
    pub fn is_regolith_active(&self) -> bool {
        self.chain_spec.is_regolith_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if ecotone is active for the payload.
    pub fn is_ecotone_active(&self) -> bool {
        self.chain_spec.is_ecotone_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if canyon is active for the payload.
    pub fn is_canyon_active(&self) -> bool {
        self.chain_spec.is_canyon_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if holocene is active for the payload.
    pub fn is_holocene_active(&self) -> bool {
        self.chain_spec.is_holocene_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if the fees are higher than the previous payload.
    pub fn is_better_payload(&self, total_fees: U256) -> bool {
        is_better_payload(self.best_payload.as_ref(), total_fees)
    }

    /// Commits the withdrawals from the payload attributes to the state.
    pub fn commit_withdrawals<DB>(
        &self,
        db: &mut State<DB>,
    ) -> Result<WithdrawalsOutcome, ProviderError>
    where
        DB: Database<Error = ProviderError>,
    {
        commit_withdrawals(
            db,
            &self.chain_spec,
            self.attributes().payload_attributes.timestamp,
            self.attributes().payload_attributes.withdrawals.clone(),
        )
    }

    /// Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
    /// blocks will always have at least a single transaction in them (the L1 info transaction),
    /// so we can safely assume that this will always be triggered upon the transition and that
    /// the above check for empty blocks will never be hit on OP chains.
    pub fn ensure_create2_deployer<DB>(&self, db: &mut State<DB>) -> Result<(), PayloadBuilderError>
    where
        DB: Database,
        DB::Error: Display,
    {
        reth_optimism_evm::ensure_create2_deployer(
            self.chain_spec.clone(),
            self.attributes().payload_attributes.timestamp,
            db,
        )
        .map_err(|err| {
            warn!(target: "payload_builder", %err, "missing create2 deployer, skipping block.");
            PayloadBuilderError::other(OptimismPayloadBuilderError::ForceCreate2DeployerFail)
        })
    }
}

impl<EvmConfig> OpPayloadBuilderCtx<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    /// apply eip-4788 pre block contract call
    pub fn apply_pre_beacon_root_contract_call<DB>(
        &self,
        db: &mut DB,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        SystemCaller::new(self.evm_config.clone(), self.chain_spec.clone())
            .pre_block_beacon_root_contract_call(
                db,
                &self.initialized_cfg,
                &self.initialized_block_env,
                self.attributes().payload_attributes.parent_beacon_block_root,
            )
            .map_err(|err| {
                warn!(target: "payload_builder",
                    parent_header=%self.parent().hash(),
                    %err,
                    "failed to apply beacon root contract call for payload"
                );
                PayloadBuilderError::Internal(err.into())
            })?;

        Ok(())
    }

    /// Executes all sequencer transactions that are included in the payload attributes.
    pub fn execute_sequencer_transactions<DB>(
        &self,
        db: &mut State<DB>,
    ) -> Result<ExecutionInfo, PayloadBuilderError>
    where
        DB: Database<Error = ProviderError>,
    {
        let mut info = ExecutionInfo::with_capacity(self.attributes().transactions.len());

        for sequencer_tx in &self.attributes().transactions {
            // A sequencer's block should never contain blob transactions.
            if sequencer_tx.value().is_eip4844() {
                return Err(PayloadBuilderError::other(
                    OptimismPayloadBuilderError::BlobTransactionRejected,
                ))
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

            // Cache the depositor account prior to the state transition for the deposit nonce.
            //
            // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
            // were not introduced in Bedrock. In addition, regular transactions don't have deposit
            // nonces, so we don't need to touch the DB for those.
            let depositor = (self.is_regolith_active() && sequencer_tx.is_deposit())
                .then(|| {
                    db.load_cache_account(sequencer_tx.signer())
                        .map(|acc| acc.account_info().unwrap_or_default())
                })
                .transpose()
                .map_err(|_| {
                    PayloadBuilderError::other(OptimismPayloadBuilderError::AccountLoadFailed(
                        sequencer_tx.signer(),
                    ))
                })?;

            let env = EnvWithHandlerCfg::new_with_cfg_env(
                self.initialized_cfg.clone(),
                self.initialized_block_env.clone(),
                self.evm_config.tx_env(sequencer_tx.as_signed(), sequencer_tx.signer()),
            );

            let mut evm = self.evm_config.evm_with_env(&mut *db, env);

            let ResultAndState { result, state } = match evm.transact() {
                Ok(res) => res,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            trace!(target: "payload_builder", %err, ?sequencer_tx, "Error in sequencer transaction, skipping.");
                            continue
                        }
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(PayloadBuilderError::EvmExecutionError(err))
                        }
                    }
                }
            };

            // to release the db reference drop evm.
            drop(evm);
            // commit changes
            db.commit(state);

            let gas_used = result.gas_used();

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            info.cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            info.receipts.push(Some(Receipt {
                tx_type: sequencer_tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used: info.cumulative_gas_used,
                logs: result.into_logs().into_iter().map(Into::into).collect(),
                deposit_nonce: depositor.map(|account| account.nonce),
                // The deposit receipt version was introduced in Canyon to indicate an update to how
                // receipt hashes should be computed when set. The state transition process
                // ensures this is only set for post-Canyon deposit transactions.
                deposit_receipt_version: self.is_canyon_active().then_some(1),
            }));

            // append sender and transaction to the respective lists
            info.executed_senders.push(sequencer_tx.signer());
            info.executed_transactions.push(sequencer_tx.into_signed());
        }

        Ok(info)
    }

    /// Executes the given best transactions and updates the execution info
    pub fn execute_best_transactions<DB, Pool>(
        &self,
        info: &mut ExecutionInfo,
        db: &mut State<DB>,
        mut best_txs: BestTransactionsFor<Pool>,
    ) -> Result<Option<BuildOutcomeKind<OpBuiltPayload>>, PayloadBuilderError>
    where
        DB: Database<Error = ProviderError>,
        Pool: TransactionPool,
    {
        let block_gas_limit = self.block_gas_limit();
        let base_fee = self.base_fee();
        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if info.cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as
                // invalid which also removes all dependent transaction from
                // the iterator before we can continue
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            // A sequencer's block should never contain blob or deposit transactions from the pool.
            if pool_tx.is_eip4844() || pool_tx.tx_type() == TxType::Deposit as u8 {
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            // check if the job was cancelled, if so we can exit early
            if self.cancel.is_cancelled() {
                return Ok(Some(BuildOutcomeKind::Cancelled))
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_recovered_transaction();
            let env = EnvWithHandlerCfg::new_with_cfg_env(
                self.initialized_cfg.clone(),
                self.initialized_block_env.clone(),
                self.evm_config.tx_env(tx.as_signed(), tx.signer()),
            );

            // Configure the environment for the block.
            let mut evm = self.evm_config.evm_with_env(&mut *db, env);

            let ResultAndState { result, state } = match evm.transact() {
                Ok(res) => res,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
                                // if the nonce is too low, we can skip this transaction
                                trace!(target: "payload_builder", %err, ?tx, "skipping nonce too low transaction");
                            } else {
                                // if the transaction is invalid, we can skip it and all of its
                                // descendants
                                trace!(target: "payload_builder", %err, ?tx, "skipping invalid transaction and its descendants");
                                best_txs.mark_invalid(&pool_tx);
                            }

                            continue
                        }
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(PayloadBuilderError::EvmExecutionError(err))
                        }
                    }
                }
            };
            // drop evm so db is released.
            drop(evm);
            // commit changes
            db.commit(state);

            let gas_used = result.gas_used();

            // add gas used by the transaction to cumulative gas used, before creating the
            // receipt
            info.cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            info.receipts.push(Some(Receipt {
                tx_type: tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used: info.cumulative_gas_used,
                logs: result.into_logs().into_iter().map(Into::into).collect(),
                deposit_nonce: None,
                deposit_receipt_version: None,
            }));

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(Some(base_fee))
                .expect("fee is always valid; execution succeeded");
            info.total_fees += U256::from(miner_fee) * U256::from(gas_used);

            // append sender and transaction to the respective lists
            info.executed_senders.push(tx.signer());
            info.executed_transactions.push(tx.into_signed());
        }

        Ok(None)
    }
}

/// Extracts the Holocene 1599 parameters from the encoded form:
/// <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/holocene/exec-engine.md#eip1559params-encoding>
pub fn decode_eip_1559_params(eip_1559_params: B64) -> (u32, u32) {
    let denominator: [u8; 4] = eip_1559_params.0[..4].try_into().expect("sufficient length");
    let elasticity: [u8; 4] = eip_1559_params.0[4..8].try_into().expect("sufficient length");

    (u32::from_be_bytes(elasticity), u32::from_be_bytes(denominator))
}
