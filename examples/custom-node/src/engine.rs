use std::sync::Arc;

use crate::{
    chainspec::CustomChainSpec,
    evm::CustomEvmConfig,
    primitives::{CustomHeader, CustomNodePrimitives},
    txpool::CustomTxPool,
};
use alloy_consensus::{
    proofs::calculate_transaction_root, Block, BlockBody, Header, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::merge::BEACON_NONCE;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use op_alloy_rpc_types_engine::{OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4};
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, BuildOutcomeKind, PayloadBuilder, PayloadConfig,
};
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates};
use reth_chainspec::ChainSpecProvider;
use reth_execution_types::ExecutionOutcome;
use reth_node_api::{
    Block as _, BuiltPayload, EngineTypes, ExecutionData, NodePrimitives, PayloadAttributes,
    PayloadBuilderAttributes, PayloadBuilderError, PayloadTypes,
};
use reth_optimism_consensus::calculate_receipt_root_no_memo_optimism;
use reth_optimism_node::{
    BasicOpReceiptBuilder, OpBuiltPayload, OpPayloadAttributes, OpPayloadBuilder,
    OpPayloadBuilderAttributes,
};
use reth_optimism_payload_builder::builder::{
    ExecutedPayload, OpBuilder, OpPayloadBuilderCtx, OpPayloadTransactions,
};
use reth_optimism_primitives::{OpBlock, OpReceipt, OpTransactionSigned};
use reth_payload_util::{NoopPayloadTransactions, PayloadTransactions};
use reth_primitives_traits::{node::AnyNodePrimitives, RecoveredBlock, SealedBlock};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{BestTransactionsAttributes, TransactionPool};
use revm::db::State;
use revm_primitives::U256;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct CustomEngineTypes;

#[derive(Debug, Clone)]
pub struct CustomBuiltPayload(OpBuiltPayload<CustomNodePrimitives>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomExecutionData {
    inner: ExecutionData,
    extension: u64,
}

impl reth_node_api::ExecutionPayload for CustomExecutionData {
    fn block_hash(&self) -> revm_primitives::B256 {
        self.inner.block_hash()
    }

    fn block_number(&self) -> u64 {
        self.inner.block_number()
    }

    fn parent_hash(&self) -> revm_primitives::B256 {
        self.inner.parent_hash()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPayloadAttributes {
    #[serde(flatten)]
    inner: OpPayloadAttributes,
    extension: u64,
}

impl PayloadAttributes for CustomPayloadAttributes {
    fn parent_beacon_block_root(&self) -> Option<revm_primitives::B256> {
        self.inner.parent_beacon_block_root()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.inner.withdrawals()
    }
}

#[derive(Debug, Clone)]
pub struct CustomPayloadBuilderAttributes {
    inner: OpPayloadBuilderAttributes<OpTransactionSigned>,
    extension: u64,
}

impl PayloadBuilderAttributes for CustomPayloadBuilderAttributes {
    type RpcPayloadAttributes = CustomPayloadAttributes;
    type Error = alloy_rlp::Error;

    fn try_new(
        parent: revm_primitives::B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
        version: u8,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        let CustomPayloadAttributes { inner, extension } = rpc_payload_attributes;

        Ok(Self { inner: OpPayloadBuilderAttributes::try_new(parent, inner, version)?, extension })
    }

    fn parent(&self) -> revm_primitives::B256 {
        self.inner.parent()
    }

    fn parent_beacon_block_root(&self) -> Option<revm_primitives::B256> {
        self.inner.parent_beacon_block_root()
    }

    fn payload_id(&self) -> alloy_rpc_types_engine::PayloadId {
        self.inner.payload_id()
    }

    fn prev_randao(&self) -> revm_primitives::B256 {
        self.inner.prev_randao()
    }

    fn suggested_fee_recipient(&self) -> revm_primitives::Address {
        self.inner.suggested_fee_recipient()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> &alloy_eips::eip4895::Withdrawals {
        self.inner.withdrawals()
    }
}

impl BuiltPayload for CustomBuiltPayload {
    type Primitives = CustomNodePrimitives;

    fn block(&self) -> &SealedBlock<<Self::Primitives as NodePrimitives>::Block> {
        self.0.block()
    }

    fn executed_block(&self) -> Option<ExecutedBlockWithTrieUpdates<Self::Primitives>> {
        self.0.executed_block()
    }

    fn fees(&self) -> U256 {
        self.0.fees()
    }

    fn requests(&self) -> Option<alloy_eips::eip7685::Requests> {
        self.0.requests()
    }
}

impl From<CustomBuiltPayload>
    for alloy_consensus::Block<<CustomNodePrimitives as NodePrimitives>::SignedTx>
{
    fn from(value: CustomBuiltPayload) -> Self {
        value.0.into_sealed_block().into_block().map_header(|header| header.inner)
    }
}

impl From<CustomBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self::from_block_unchecked(value.block().hash(), &value.into())
    }
}

impl From<CustomBuiltPayload> for ExecutionPayloadV2 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self::from_block_unchecked(value.block().hash(), &value.into())
    }
}

impl From<CustomBuiltPayload> for OpExecutionPayloadEnvelopeV3 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self {
            block_value: value.fees(),
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            // No blobs for OP.
            blobs_bundle: BlobsBundleV1 { blobs: vec![], commitments: vec![], proofs: vec![] },
            parent_beacon_block_root: value.0.block().parent_beacon_block_root.unwrap_or_default(),
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.0.block().hash(),
                &value.into(),
            ),
        }
    }
}

impl From<CustomBuiltPayload> for OpExecutionPayloadEnvelopeV4 {
    fn from(value: CustomBuiltPayload) -> Self {
        Self {
            block_value: value.fees(),
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            // No blobs for OP.
            blobs_bundle: BlobsBundleV1 { blobs: vec![], commitments: vec![], proofs: vec![] },
            parent_beacon_block_root: value.0.block().parent_beacon_block_root.unwrap_or_default(),
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.0.block().hash(),
                &value.into(),
            ),
            execution_requests: vec![],
        }
    }
}

impl PayloadTypes for CustomEngineTypes {
    type BuiltPayload = CustomBuiltPayload;
    type PayloadAttributes = CustomPayloadAttributes;
    type PayloadBuilderAttributes = CustomPayloadBuilderAttributes;
}

impl EngineTypes for CustomEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadV2;
    type ExecutionPayloadEnvelopeV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = OpExecutionPayloadEnvelopeV4;
    type ExecutionData = CustomExecutionData;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let extension = block.header().extension;
        let block_hash = block.hash();
        let block = block.into_block().map_header(|header| header.inner);
        let (payload, sidecar) = ExecutionPayload::from_block_unchecked(block_hash, &block);
        CustomExecutionData { inner: ExecutionData { payload, sidecar }, extension }
    }
}

#[derive(Debug, Clone)]
pub struct CustomPayloadBuilder<Provider> {
    inner:
        OpPayloadBuilder<CustomTxPool<Provider>, Provider, CustomEvmConfig, CustomNodePrimitives>,
}

impl<Provider> CustomPayloadBuilder<Provider> {
    pub fn new(
        pool: CustomTxPool<Provider>,
        provider: Provider,
        chain_spec: Arc<CustomChainSpec>,
    ) -> Self {
        Self {
            inner: OpPayloadBuilder::new(
                pool,
                provider,
                CustomEvmConfig::new(chain_spec),
                BasicOpReceiptBuilder::default(),
            ),
        }
    }
}

impl<Provider> CustomPayloadBuilder<Provider>
where
    Provider: Clone
        + StateProviderFactory
        + BlockReaderIdExt
        + ChainSpecProvider<ChainSpec = CustomChainSpec>
        + 'static,
{
    fn build_payload<Txs>(
        &self,
        args: BuildArguments<CustomPayloadBuilderAttributes, CustomBuiltPayload>,
        txs: impl FnOnce(BestTransactionsAttributes) -> Txs + Send + Sync,
    ) -> Result<BuildOutcome<CustomBuiltPayload>, PayloadBuilderError>
    where
        Txs: PayloadTransactions<
            Transaction = <CustomTxPool<Provider> as TransactionPool>::Transaction,
        >,
    {
        let BuildArguments { mut cached_reads, config, cancel, best_payload } = args;
        let builder = OpBuilder::new(txs);

        let ctx = OpPayloadBuilderCtx {
            evm_config: self.inner.evm_config.clone(),
            da_config: self.inner.config.da_config.clone(),
            chain_spec: self.inner.client.chain_spec(),
            config: PayloadConfig {
                parent_header: config.parent_header.clone(),
                attributes: config.attributes.inner.clone(),
            },
            evm_env: self
                .inner
                .evm_env(&config.attributes.inner, &config.parent_header)
                .map_err(PayloadBuilderError::other)?,
            cancel: cancel.clone(),
            best_payload: best_payload.map(|payload| payload.0),
            receipt_builder: self.inner.receipt_builder.clone(),
        };

        let state_provider = self.inner.client.state_by_block_hash(ctx.parent().hash())?;
        let state = StateProviderDatabase::new(state_provider);

        let (executed, bundle_state, state) = if ctx.attributes().no_tx_pool {
            let mut db = State::builder().with_database(state).with_bundle_update().build();
            (builder.execute(&mut db, &ctx)?, db.take_bundle(), db.database)
        } else {
            // sequencer mode we can reuse cachedreads from previous runs
            let mut db = State::builder()
                .with_database(cached_reads.as_db_mut(state))
                .with_bundle_update()
                .build();
            (builder.execute(&mut db, &ctx)?, db.take_bundle(), db.database.db)
        };

        let ExecutedPayload { info, withdrawals_root } = match executed {
            BuildOutcomeKind::Better { payload } | BuildOutcomeKind::Freeze(payload) => payload,
            BuildOutcomeKind::Cancelled => {
                return Ok(BuildOutcomeKind::Cancelled.with_cached_reads(cached_reads))
            }
            BuildOutcomeKind::Aborted { fees } => {
                return Ok(BuildOutcomeKind::Aborted { fees }.with_cached_reads(cached_reads))
            }
        };

        let block_number = ctx.block_number();
        let execution_outcome =
            ExecutionOutcome::new(bundle_state, vec![info.receipts], block_number, Vec::new());
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
        let hashed_state = state.hashed_post_state(execution_outcome.state());
        let (state_root, trie_output) = state.state_root_with_updates(hashed_state.clone())?;

        // create the block header
        let transactions_root = calculate_transaction_root(&info.executed_transactions);

        // OP doesn't support blobs/EIP-4844.
        // https://specs.optimism.io/protocol/exec-engine.html#ecotone-disable-blob-transactions
        // Need [Some] or [None] based on hardfork to match block hash.
        let (excess_blob_gas, blob_gas_used) = ctx.blob_fields();
        let extra_data = ctx.extra_data()?;

        let header = CustomHeader {
            inner: Header {
                parent_hash: ctx.parent().hash(),
                ommers_hash: EMPTY_OMMER_ROOT_HASH,
                beneficiary: ctx.evm_env.block_env.coinbase,
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
                parent_beacon_block_root: ctx
                    .attributes()
                    .payload_attributes
                    .parent_beacon_block_root,
                blob_gas_used,
                excess_blob_gas,
                requests_hash: None,
            },
            extension: config.attributes.extension,
        };

        // seal the block
        let block = Block::new(
            header,
            BlockBody {
                transactions: info.executed_transactions,
                ommers: vec![],
                withdrawals: ctx.withdrawals().cloned(),
            },
        );

        let sealed_block = Arc::new(block.seal_slow());

        // create the executed block data
        let executed: ExecutedBlockWithTrieUpdates<CustomNodePrimitives> =
            ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock {
                    recovered_block: Arc::new(RecoveredBlock::new_sealed(
                        sealed_block.as_ref().clone(),
                        info.executed_senders,
                    )),
                    execution_output: Arc::new(execution_outcome),
                    hashed_state: Arc::new(hashed_state),
                },
                trie: Arc::new(trie_output),
            };

        let no_tx_pool = ctx.attributes().no_tx_pool;

        let payload = CustomBuiltPayload(OpBuiltPayload::new(
            ctx.payload_id(),
            sealed_block,
            info.total_fees,
            Some(executed),
        ));

        let outcome = if no_tx_pool {
            // if `no_tx_pool` is set only transactions from the payload attributes will be included
            // in the payload. In other words, the payload is deterministic and we can
            // freeze it once we've successfully built it.
            BuildOutcomeKind::Freeze(payload)
        } else {
            BuildOutcomeKind::Better { payload }
        };

        Ok(outcome.with_cached_reads(cached_reads))
    }
}

impl<Provider> PayloadBuilder for CustomPayloadBuilder<Provider>
where
    Provider: Clone
        + StateProviderFactory
        + BlockReaderIdExt
        + ChainSpecProvider<ChainSpec = CustomChainSpec>
        + 'static,
{
    type Attributes = CustomPayloadBuilderAttributes;
    type BuiltPayload = CustomBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        self.build_payload(args, |attrs| {
            self.inner.best_transactions.best_transactions(self.inner.pool.clone(), attrs)
        })
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, CustomHeader>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let args = BuildArguments {
            cached_reads: Default::default(),
            config,
            cancel: Default::default(),
            best_payload: None,
        };
        self.build_payload(args, |_| NoopPayloadTransactions::default())?
            .into_payload()
            .ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}
