//! Scroll's payload builder implementation.

use super::{PayloadBuildingBaseFeeProvider, ScrollPayloadBuilderError};
use crate::config::{PayloadBuildingBreaker, ScrollBuilderConfig};

use alloy_consensus::{Transaction, Typed2718};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use core::fmt::Debug;
use reth_basic_payload_builder::{
    is_better_payload, BuildArguments, BuildOutcome, BuildOutcomeKind, MissingPayloadBehaviour,
    PayloadBuilder, PayloadConfig,
};
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates, ExecutedTrieUpdates};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_evm::{
    block::{BlockExecutionError, BlockValidationError},
    execute::{BlockBuilder, BlockBuilderOutcome, ProviderError},
    ConfigureEvm, Database, Evm,
};
use reth_execution_types::ExecutionOutcome;
use reth_payload_builder::PayloadId;
use reth_payload_primitives::{PayloadBuilderAttributes, PayloadBuilderError};
use reth_payload_util::{BestPayloadTransactions, NoopPayloadTransactions, PayloadTransactions};
use reth_primitives_traits::{
    NodePrimitives, RecoveredBlock, SealedHeader, SignedTransaction, TxTy,
};
use reth_revm::{cancelled::CancelOnDrop, database::StateProviderDatabase, db::State};
use reth_scroll_engine_primitives::{ScrollBuiltPayload, ScrollPayloadBuilderAttributes};
use reth_scroll_evm::ScrollNextBlockEnvAttributes;
use reth_scroll_primitives::{ScrollPrimitives, ScrollTransactionSigned};
use reth_storage_api::{StateProvider, StateProviderFactory};
use reth_transaction_pool::{BestTransactionsAttributes, PoolTransaction, TransactionPool};
use revm::context::{Block, BlockEnv};
use scroll_alloy_hardforks::ScrollHardforks;
use std::{boxed::Box, sync::Arc, vec, vec::Vec};

/// A type that returns the [`PayloadTransactions`] that should be included in the pool.
pub trait ScrollPayloadTransactions<Transaction>: Clone + Send + Sync + Unpin + 'static {
    /// Returns an iterator that yields the transaction in the order they should get included in the
    /// new payload.
    fn best_transactions<Pool: TransactionPool<Transaction = Transaction>>(
        &self,
        pool: Pool,
        attr: BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = Transaction>;
}

impl<T: PoolTransaction> ScrollPayloadTransactions<T> for () {
    fn best_transactions<Pool: TransactionPool<Transaction = T>>(
        &self,
        pool: Pool,
        attr: BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = T> {
        BestPayloadTransactions::new(pool.best_transactions_with_attributes(attr))
    }
}

/// Scroll's payload builder.
#[derive(Clone, Debug)]
pub struct ScrollPayloadBuilder<Pool, Client, Evm, Txs = ()> {
    /// The type responsible for creating the evm.
    pub evm_config: Evm,
    /// Transaction pool.
    pub pool: Pool,
    /// Node client
    pub client: Client,
    /// The type responsible for yielding the best transactions to include in a payload.
    pub best_transactions: Txs,
    /// Payload builder configuration.
    pub builder_config: ScrollBuilderConfig,
}

impl<Pool, Evm, Client> ScrollPayloadBuilder<Pool, Client, Evm> {
    /// Creates a new [`ScrollPayloadBuilder`].
    pub const fn new(
        pool: Pool,
        evm_config: Evm,
        client: Client,
        builder_config: ScrollBuilderConfig,
    ) -> Self {
        Self { evm_config, pool, client, best_transactions: (), builder_config }
    }
}

impl<Pool, Client, Evm, Txs> ScrollPayloadBuilder<Pool, Client, Evm, Txs> {
    /// Configures the type responsible for yielding the transactions that should be included in the
    /// payload.
    pub fn with_transactions<T>(
        self,
        best_transactions: T,
    ) -> ScrollPayloadBuilder<Pool, Client, Evm, T> {
        let Self { evm_config, pool, client, builder_config, .. } = self;
        ScrollPayloadBuilder { evm_config, pool, client, best_transactions, builder_config }
    }
}

impl<Pool, Client, Evm, T> ScrollPayloadBuilder<Pool, Client, Evm, T>
where
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthChainSpec + ScrollHardforks>,
    Evm:
        ConfigureEvm<Primitives = ScrollPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
{
    /// Constructs a Scroll payload from the transactions sent via the
    /// Payload attributes by the sequencer. If the `no_tx_pool` argument is passed in
    /// the payload attributes, the transaction pool will be ignored and the only transactions
    /// included in the payload will be those sent through the attributes.
    ///
    /// Given build arguments including a Scroll client, transaction pool,
    /// and configuration, this function creates a transaction payload. Returns
    /// a result indicating success with the payload or an error in case of failure.
    fn build_payload<'a, Txs>(
        &self,
        args: BuildArguments<ScrollPayloadBuilderAttributes, ScrollBuiltPayload>,
        best: impl FnOnce(BestTransactionsAttributes) -> Txs + Send + Sync + 'a,
    ) -> Result<BuildOutcome<ScrollBuiltPayload>, PayloadBuilderError>
    where
        Txs: PayloadTransactions<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>,
    {
        let BuildArguments { mut cached_reads, config, cancel, best_payload } = args;

        let ctx = ScrollPayloadBuilderCtx {
            evm_config: self.evm_config.clone(),
            chain_spec: self.client.chain_spec(),
            config,
            cancel,
            best_payload,
        };

        let builder = ScrollBuilder::new(best);

        let state_provider = self.client.state_by_block_hash(ctx.parent().hash())?;
        let state = StateProviderDatabase::new(&state_provider);

        if ctx.attributes().no_tx_pool {
            builder.build(state, &state_provider, ctx, &self.builder_config)
        } else {
            // sequencer mode we can reuse cachedreads from previous runs
            builder.build(cached_reads.as_db_mut(state), &state_provider, ctx, &self.builder_config)
        }
        .map(|out| out.with_cached_reads(cached_reads))
    }
}

/// Implementation of the [`PayloadBuilder`] trait for [`ScrollPayloadBuilder`].
impl<Pool, Client, Evm, Txs> PayloadBuilder for ScrollPayloadBuilder<Pool, Client, Evm, Txs>
where
    Client:
        StateProviderFactory + ChainSpecProvider<ChainSpec: EthChainSpec + ScrollHardforks> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>,
    Evm:
        ConfigureEvm<Primitives = ScrollPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    Txs: ScrollPayloadTransactions<Pool::Transaction>,
{
    type Attributes = ScrollPayloadBuilderAttributes;
    type BuiltPayload = ScrollBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let pool = self.pool.clone();
        self.build_payload(args, |attrs| self.best_transactions.best_transactions(pool, attrs))
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        // we want to await the job that's already in progress because that should be returned as
        // is, there's no benefit in racing another job
        MissingPayloadBehaviour::AwaitInProgress
    }

    // NOTE: this should only be used for testing purposes because this doesn't have access to L1
    // system txs, hence on_missing_payload we return [MissingPayloadBehaviour::AwaitInProgress].
    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let args = BuildArguments {
            config,
            cached_reads: Default::default(),
            cancel: Default::default(),
            best_payload: None,
        };
        self.build_payload(args, |_| NoopPayloadTransactions::<Pool::Transaction>::default())?
            .into_payload()
            .ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}

/// A builder for a new payload.
pub struct ScrollBuilder<'a, Txs> {
    /// Yields the best transaction to include if transactions from the mempool are allowed.
    best: Box<dyn FnOnce(BestTransactionsAttributes) -> Txs + 'a>,
}

impl<'a, Txs> ScrollBuilder<'a, Txs> {
    /// Creates a new [`ScrollBuilder`].
    pub fn new(best: impl FnOnce(BestTransactionsAttributes) -> Txs + Send + Sync + 'a) -> Self {
        Self { best: Box::new(best) }
    }
}

impl<'a, Txs> std::fmt::Debug for ScrollBuilder<'a, Txs> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollBuilder").finish()
    }
}

impl<Txs> ScrollBuilder<'_, Txs> {
    /// Builds the payload on top of the state.
    pub fn build<EvmConfig, ChainSpec>(
        self,
        db: impl Database<Error = ProviderError>,
        state_provider: impl StateProvider,
        ctx: ScrollPayloadBuilderCtx<EvmConfig, ChainSpec>,
        builder_config: &ScrollBuilderConfig,
    ) -> Result<BuildOutcomeKind<ScrollBuiltPayload>, PayloadBuilderError>
    where
        EvmConfig: ConfigureEvm<
            Primitives = ScrollPrimitives,
            NextBlockEnvCtx = ScrollNextBlockEnvAttributes,
        >,
        ChainSpec: EthChainSpec + ScrollHardforks,
        Txs: PayloadTransactions<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>,
    {
        let Self { best } = self;
        tracing::debug!(target: "payload_builder", id=%ctx.payload_id(), parent_header = ?ctx.parent().hash(), parent_number = ctx.parent().number, "building new payload");
        let breaker = builder_config.breaker();

        let mut db = State::builder().with_database(db).with_bundle_update().build();

        let mut builder = ctx.block_builder(&mut db, builder_config)?;

        // 1. apply pre-execution changes
        builder.apply_pre_execution_changes().map_err(|err| {
            tracing::warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
            PayloadBuilderError::Internal(err.into())
        })?;

        // 2. execute sequencer transactions
        let mut info = ctx.execute_sequencer_transactions(&mut builder)?;

        // 3. if mem pool transactions are requested we execute them
        if !ctx.attributes().no_tx_pool {
            let best_txs = best(ctx.best_transaction_attributes(builder.evm_mut().block()));
            if ctx.execute_best_transactions(&mut info, &mut builder, best_txs, breaker)?.is_some()
            {
                return Ok(BuildOutcomeKind::Cancelled);
            }

            // check if the new payload is even more valuable
            if !ctx.is_better_payload(info.total_fees) {
                // can skip building the block
                return Ok(BuildOutcomeKind::Aborted { fees: info.total_fees })
            }
        }

        let BlockBuilderOutcome { execution_result, hashed_state, trie_updates, mut block } =
            builder.finish(state_provider)?;

        // set the block fields using the hints from the payload attributes.
        let (mut scroll_block, senders) = block.split();
        scroll_block = scroll_block.map_header(|mut header| {
            if let Some(extra_data) = &ctx.config.attributes.block_data_hint.extra_data {
                header.extra_data = extra_data.clone();
            }
            if let Some(state_root) = ctx.config.attributes.block_data_hint.state_root {
                header.state_root = state_root;
            }
            if let Some(coinbase) = ctx.config.attributes.block_data_hint.coinbase {
                header.beneficiary = coinbase;
            }
            if let Some(nonce) = ctx.config.attributes.block_data_hint.nonce {
                header.nonce = nonce.into()
            }
            if let Some(difficulty) = ctx.config.attributes.block_data_hint.difficulty {
                header.difficulty = difficulty;
            }
            header
        });
        block = RecoveredBlock::new_unhashed(scroll_block, senders);

        let sealed_block = Arc::new(block.sealed_block().clone());
        tracing::debug!(target: "payload_builder", id=%ctx.attributes().payload_id(), sealed_block_header = ?sealed_block.header(), "sealed built block");

        let execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![execution_result.receipts],
            block.number,
            Vec::new(),
        );

        // create the executed block data
        let executed: ExecutedBlockWithTrieUpdates<ScrollPrimitives> =
            ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock {
                    recovered_block: Arc::new(block),
                    execution_output: Arc::new(execution_outcome),
                    hashed_state: Arc::new(hashed_state),
                },
                trie: ExecutedTrieUpdates::Present(Arc::new(trie_updates)),
            };

        let no_tx_pool = ctx.attributes().no_tx_pool;

        let payload = ScrollBuiltPayload::new(
            ctx.payload_id(),
            sealed_block,
            Some(executed),
            info.total_fees,
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

/// Container type that holds all necessities to build a new payload.
#[derive(Debug)]
pub struct ScrollPayloadBuilderCtx<Evm: ConfigureEvm, ChainSpec> {
    /// The type that knows how to perform system calls and configure the evm.
    pub evm_config: Evm,
    /// The chainspec
    pub chain_spec: ChainSpec,
    /// How to build the payload.
    pub config: PayloadConfig<ScrollPayloadBuilderAttributes>,
    /// Marker to check whether the job has been cancelled.
    pub cancel: CancelOnDrop,
    /// The currently best payload.
    pub best_payload: Option<ScrollBuiltPayload>,
}

impl<Evm, ChainSpec> ScrollPayloadBuilderCtx<Evm, ChainSpec>
where
    Evm:
        ConfigureEvm<Primitives = ScrollPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    ChainSpec: EthChainSpec + ScrollHardforks,
{
    /// Returns the parent block the payload will be build on.
    #[allow(clippy::missing_const_for_fn)]
    pub fn parent(&self) -> &SealedHeader {
        &self.config.parent_header
    }

    /// Returns the builder attributes.
    pub const fn attributes(&self) -> &ScrollPayloadBuilderAttributes {
        &self.config.attributes
    }

    /// Returns the current fee settings for transactions from the mempool
    pub fn best_transaction_attributes(&self, block_env: &BlockEnv) -> BestTransactionsAttributes {
        BestTransactionsAttributes::new(
            block_env.basefee,
            block_env.blob_gasprice().map(|p| p as u64),
        )
    }

    /// Returns the unique id for this payload job.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes().payload_id()
    }

    /// Returns true if the fees are higher than the previous payload.
    pub fn is_better_payload(&self, total_fees: U256) -> bool {
        is_better_payload(self.best_payload.as_ref(), total_fees)
    }

    /// Prepares a [`BlockBuilder`] for the next block.
    pub fn block_builder<'a, DB: Database>(
        &'a self,
        db: &'a mut State<DB>,
        builder_config: &ScrollBuilderConfig,
    ) -> Result<impl BlockBuilder<Primitives = Evm::Primitives> + 'a, PayloadBuilderError> {
        // get the base fee for the attributes.
        let base_fee: u64 = if self.chain_spec.is_curie_active_at_block(self.parent().number + 1) {
            db.payload_building_base_fee()
                .map_err(|err| PayloadBuilderError::Other(Box::new(err)))?
                .try_into()
                .expect("base fee limited to 10_000_000_000")
        } else {
            0
        };

        self.evm_config
            .builder_for_next_block(
                db,
                self.parent(),
                ScrollNextBlockEnvAttributes {
                    timestamp: self.attributes().timestamp(),
                    suggested_fee_recipient: self.attributes().suggested_fee_recipient(),
                    gas_limit: self.attributes().gas_limit.unwrap_or(builder_config.gas_limit),
                    base_fee,
                },
            )
            .map_err(PayloadBuilderError::other)
    }
}

impl<Evm, ChainSpec> ScrollPayloadBuilderCtx<Evm, ChainSpec>
where
    Evm:
        ConfigureEvm<Primitives = ScrollPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    ChainSpec: EthChainSpec + ScrollHardforks,
{
    /// Executes all sequencer transactions that are included in the payload attributes.
    pub fn execute_sequencer_transactions(
        &self,
        builder: &mut impl BlockBuilder<Primitives = Evm::Primitives>,
    ) -> Result<ExecutionInfo, PayloadBuilderError> {
        let mut info = ExecutionInfo::new();

        for sequencer_tx in &self.attributes().transactions {
            // A sequencer's block should never contain blob transactions.
            if sequencer_tx.value().is_eip4844() {
                return Err(PayloadBuilderError::other(
                    ScrollPayloadBuilderError::BlobTransactionRejected,
                ))
            }

            // Convert the transaction to a [RecoveredTx]. This is
            // purely for the purposes of utilizing the `evm_config.tx_env`` function.
            // Deposit transactions do not have signatures, so if the tx is a deposit, this
            // will just pull in its `from` address.
            let sequencer_tx = sequencer_tx.value().try_clone_into_recovered().map_err(|_| {
                PayloadBuilderError::other(ScrollPayloadBuilderError::TransactionEcRecoverFailed)
            })?;

            let gas_used = match builder.execute_transaction(sequencer_tx.clone()) {
                Ok(gas_used) => gas_used,
                Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                    error,
                    ..
                })) => {
                    tracing::trace!(target: "payload_builder", %error, ?sequencer_tx, "Error in sequencer transaction, skipping.");
                    continue
                }
                Err(err) => {
                    // this is an error that we should treat as fatal for this attempt
                    return Err(PayloadBuilderError::EvmExecutionError(Box::new(err)))
                }
            };

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            info.cumulative_gas_used += gas_used;
        }

        Ok(info)
    }

    /// Executes the given best transactions and updates the execution info.
    ///
    /// Returns `Ok(Some(())` if the job was cancelled.
    pub fn execute_best_transactions(
        &self,
        info: &mut ExecutionInfo,
        builder: &mut impl BlockBuilder<Primitives = Evm::Primitives>,
        mut best_txs: impl PayloadTransactions<
            Transaction: PoolTransaction<Consensus = TxTy<Evm::Primitives>>,
        >,
        breaker: PayloadBuildingBreaker,
    ) -> Result<Option<()>, PayloadBuilderError> {
        let block_gas_limit = builder.evm_mut().block().gas_limit;
        let base_fee = builder.evm_mut().block().basefee;

        while let Some(tx) = best_txs.next(()) {
            let tx = tx.into_consensus();
            if info.is_tx_over_limits(tx.inner(), block_gas_limit) {
                // we can't fit this transaction into the block, so we need to mark it as
                // invalid which also removes all dependent transaction from
                // the iterator before we can continue
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue
            }

            // A sequencer's block should never contain blob or deposit transactions from the pool.
            if tx.is_eip4844() || tx.is_l1_message() {
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue
            }

            // check if the job was cancelled, if so we can exit early
            if self.cancel.is_cancelled() {
                return Ok(Some(()))
            }

            // check if the execution needs to be halted.
            if breaker.should_break(info.cumulative_gas_used) {
                tracing::trace!(target: "scroll::payload_builder", ?info, "breaking execution loop");
                return Ok(None);
            }

            let gas_used = match builder.execute_transaction(tx.clone()) {
                Ok(gas_used) => gas_used,
                Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                    error,
                    ..
                })) => {
                    if error.is_nonce_too_low() {
                        // if the nonce is too low, we can skip this transaction
                        tracing::trace!(target: "payload_builder", %error, ?tx, "skipping nonce too low transaction");
                    } else {
                        // if the transaction is invalid, we can skip it and all of its
                        // descendants
                        tracing::trace!(target: "payload_builder", %error, ?tx, "skipping invalid transaction and its descendants");
                        best_txs.mark_invalid(tx.signer(), tx.nonce());
                    }
                    continue
                }
                Err(err) => {
                    // this is an error that we should treat as fatal for this attempt
                    return Err(PayloadBuilderError::EvmExecutionError(Box::new(err)))
                }
            };

            // add gas used by the transaction to cumulative gas used, before creating the
            // receipt
            info.cumulative_gas_used += gas_used;
            info.cumulative_da_bytes_used += tx.length() as u64;

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
                .expect("fee is always valid; execution succeeded");
            info.total_fees += U256::from(miner_fee) * U256::from(gas_used);
        }

        Ok(None)
    }
}

/// Holds the state after execution
#[derive(Debug)]
pub struct ExecutedPayload<N: NodePrimitives> {
    /// Tracked execution info
    pub info: ExecutionInfo,
    /// Withdrawal hash.
    pub withdrawals_root: Option<B256>,
    /// The transaction receipts.
    pub receipts: Vec<N::Receipt>,
    /// The block env used during execution.
    pub block_env: BlockEnv,
}

/// This acts as the container for executed transactions and its byproducts (receipts, gas used)
#[derive(Default, Debug)]
pub struct ExecutionInfo {
    /// All gas used so far
    pub cumulative_gas_used: u64,
    /// Estimated DA size
    pub cumulative_da_bytes_used: u64,
    /// Tracks fees from executed mempool transactions
    pub total_fees: U256,
}

impl ExecutionInfo {
    /// Create a new instance with allocated slots.
    pub const fn new() -> Self {
        Self { cumulative_gas_used: 0, cumulative_da_bytes_used: 0, total_fees: U256::ZERO }
    }

    /// Returns true if the transaction would exceed the block limits:
    /// - block gas limit: ensures the transaction still fits into the block.
    pub fn is_tx_over_limits(
        &self,
        tx: &(impl Encodable + Transaction),
        block_gas_limit: u64,
    ) -> bool {
        self.cumulative_gas_used + tx.gas_limit() > block_gas_limit
    }
}
