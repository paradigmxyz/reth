use crate::{ExecutionBuiltPayload, ExecutionEngineValidator};
use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::Sealable;
use alloy_rpc_types_engine::PayloadAttributes;
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{ChainSpec, ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks};
use reth_consensus_common::validation::MAX_RLP_BLOCK_SIZE;
use reth_errors::ConsensusError;
use reth_ethereum_payload_builder::{
    default_ethereum_payload_with, EthereumBuilderConfig, EthereumPayloadBuildOutcome,
};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{execute::BlockBuilderOutcome, ConfigureEvm, NextBlockEnvAttributes};
use reth_payload_builder::EthBuiltPayload;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderError};
use reth_primitives_traits::{RecoveredBlock, SealedHeader};
use reth_pureth_block::{
    ExecutionBlock, ExecutionBody, ExecutionForkConfig, ExecutionHeader, PayloadLimits,
    PayloadSourceError, SourcePayloadSnapshot, StateRootStatus,
};
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Clone, Debug)]
pub struct ExecutionPayloadBuilder<Pool, Client, EvmConfig> {
    client: Client,
    pool: Pool,
    evm_config: EvmConfig,
    builder_config: EthereumBuilderConfig,
    fork: ExecutionForkConfig,
    limits: PayloadLimits,
    native_chain_spec: Arc<ChainSpec>,
}

impl<Pool, Client, EvmConfig> ExecutionPayloadBuilder<Pool, Client, EvmConfig> {
    pub fn new(
        client: Client,
        pool: Pool,
        evm_config: EvmConfig,
        builder_config: EthereumBuilderConfig,
        fork: ExecutionForkConfig,
        limits: PayloadLimits,
    ) -> Self
    where
        Client: ChainSpecProvider<ChainSpec = ChainSpec>,
    {
        let native_chain_spec = client.chain_spec();
        Self { client, pool, evm_config, builder_config, fork, limits, native_chain_spec }
    }

    /// Reuses the ordinary builder with a typed state provider and the Engine's native EVM
    /// configuration. Computational native headers retain the selected parent identity.
    pub fn from_validator(
        client: Client,
        pool: Pool,
        evm_config: EvmConfig,
        builder_config: EthereumBuilderConfig,
        validator: &ExecutionEngineValidator,
    ) -> Result<Self, PayloadSourceError>
    where
        Client: ChainSpecProvider<ChainSpec: Hardforks>,
    {
        validator.validate_chain_spec(client.chain_spec().as_ref())?;
        Ok(Self {
            client,
            pool,
            evm_config,
            builder_config,
            fork: validator.fork_config(),
            limits: validator.limits(),
            native_chain_spec: validator.chain_spec().clone(),
        })
    }
}

impl<Pool, Client, EvmConfig> ExecutionPayloadBuilder<Pool, Client, EvmConfig>
where
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    fn build(
        &self,
        args: BuildArguments<PayloadAttributes, ExecutionBuiltPayload>,
        empty: bool,
    ) -> Result<BuildOutcome<ExecutionBuiltPayload>, PayloadBuilderError> {
        let active = self.fork.is_active_at_timestamp(args.config.attributes.timestamp);
        if active && self.builder_config.skip_state_root {
            return Err(PayloadBuilderError::other(PayloadSourceError::StateRootSkipped));
        }
        let parent = &args.config.parent_header;
        if parent.is_ssz() != self.fork.is_active_at_timestamp(parent.timestamp()) ||
            parent.hash_slow() != parent.hash()
        {
            return Err(PayloadBuilderError::other(PayloadSourceError::SourceMismatch));
        }
        let parent = Arc::new(SealedHeader::new(parent.as_native().clone(), parent.hash()));
        let cancel = args.cancel.clone();
        let spec = &self.native_chain_spec;
        cancel.scope(|| {
            if cancel.is_cancelled() {
                return Ok(BuildOutcome::Cancelled);
            }
            default_ethereum_payload_with(
                self.evm_config.clone(),
                self.client.clone(),
                self.pool.clone(),
                self.builder_config.clone(),
                args,
                parent,
                |attributes| {
                    if empty {
                        Box::new(std::iter::empty())
                    } else {
                        self.pool.best_transactions_with_attributes(attributes)
                    }
                },
                |built| self.finish(built, spec.as_ref()),
            )
        })
    }

    fn finish<S>(
        &self,
        built: EthereumPayloadBuildOutcome<'_>,
        spec: &S,
    ) -> Result<ExecutionBuiltPayload, PayloadBuilderError>
    where
        S: EthChainSpec<Header = Header> + EthereumHardforks,
    {
        let active = self.fork.is_active_at_timestamp(built.attributes.timestamp);
        let amsterdam = spec.is_amsterdam_active_at_timestamp(built.attributes.timestamp);
        if built.chain_id != spec.chain().id() ||
            built.amsterdam_eip8037 != amsterdam ||
            built.amsterdam_eip2780 != amsterdam
        {
            return Err(PayloadBuilderError::other(PayloadSourceError::UnsupportedFork));
        }
        let snapshot = if active {
            let attributes = NextBlockEnvAttributes {
                timestamp: built.attributes.timestamp,
                suggested_fee_recipient: built.attributes.suggested_fee_recipient,
                prev_randao: built.attributes.prev_randao,
                gas_limit: built.gas_limit,
                parent_beacon_block_root: built.attributes.parent_beacon_block_root,
                withdrawals: built.attributes.withdrawals.clone().map(Into::into),
                extra_data: built.extra_data.clone(),
                slot_number: built.attributes.slot_number,
            };
            Some(
                SourcePayloadSnapshot::from_builder_outcome(
                    &built.outcome,
                    built.parent,
                    &attributes,
                    spec,
                    if built.state_root_computed {
                        StateRootStatus::Computed
                    } else {
                        StateRootStatus::Skipped
                    },
                    self.limits,
                )
                .map_err(PayloadBuilderError::other)?,
            )
        } else {
            None
        };
        let BlockBuilderOutcome { block, execution_result, block_access_list, .. } = built.outcome;
        let requests = built.requests_active.then_some(execution_result.requests);
        let bal = block_access_list.map(|bal| bal.split().1);
        let (block, source) = if let Some(snapshot) = snapshot {
            let source = Arc::new(
                snapshot
                    .into_execution_source(
                        block,
                        execution_result.receipts,
                        spec,
                        &self.fork,
                        &AtomicBool::new(false),
                    )
                    .map_err(PayloadBuilderError::other)?,
            );
            (source.block_arc().clone(), Some(source))
        } else {
            let (block, senders) = block.split();
            let header = ExecutionHeader::legacy(block.header);
            let body =
                ExecutionBody::from_header(&header, block.body.map_ommers(ExecutionHeader::legacy))
                    .map_err(PayloadBuilderError::other)?;
            let block =
                ExecutionBlock::try_new(header, body).map_err(PayloadBuilderError::other)?;
            (Arc::new(RecoveredBlock::new_unhashed(block, senders)), None)
        };
        let inner =
            EthBuiltPayload::new(block, built.fees, requests, bal).with_sidecars(built.sidecars);
        let payload = ExecutionBuiltPayload::try_new(inner, source, &self.fork)
            .map_err(PayloadBuilderError::other)?;
        if spec.is_osaka_active_at_timestamp(payload.block().timestamp()) &&
            payload.block().rlp_length() > MAX_RLP_BLOCK_SIZE
        {
            return Err(PayloadBuilderError::other(ConsensusError::BlockTooLarge {
                rlp_length: payload.block().rlp_length(),
                max_rlp_length: MAX_RLP_BLOCK_SIZE,
            }));
        }
        Ok(payload)
    }
}

impl<Pool, Client, EvmConfig> PayloadBuilder for ExecutionPayloadBuilder<Pool, Client, EvmConfig>
where
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    type Attributes = PayloadAttributes;
    type BuiltPayload = ExecutionBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        self.build(args, false)
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        if self.builder_config.await_payload_on_missing {
            MissingPayloadBehaviour::AwaitInProgress
        } else {
            MissingPayloadBehaviour::RaceEmptyPayload
        }
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, ExecutionHeader>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        self.build(
            BuildArguments::new(
                Default::default(),
                Default::default(),
                None,
                config,
                Default::default(),
                None,
            ),
            true,
        )?
        .into_payload()
        .ok_or(PayloadBuilderError::MissingPayload)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
