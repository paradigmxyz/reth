//! Ethereum-specific tree payload validator implementation.

use crate::validator::ensure_well_formed_payload;
use alloy_rpc_types_engine::ExecutionData;
use reth_chainspec::EthereumHardforks;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_engine_primitives::{InvalidBlockHook, PayloadValidator};
use reth_engine_tree::tree::{
    payload_validator::{PayloadValidationOutcome, TreeCtx},
    precompile_cache::PrecompileCacheMap,
    EngineValidator, PayloadProcessor, TreeConfig, TreePayloadValidator,
};
use reth_ethereum_engine_primitives::EthPayloadTypes;
use reth_ethereum_primitives::{Block as EthBlock, EthPrimitives};
use reth_evm::{ConfigureEvm, SpecFor};
use reth_payload_primitives::{
    validate_execution_requests, validate_version_specific_fields, EngineApiMessageVersion,
    EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
};
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{
    BlockNumReader, BlockReader, DatabaseProviderFactory, HashedPostStateProvider, HeaderProvider,
    StateCommitmentProvider, StateProviderFactory, StateReader,
};
use reth_trie::HashedPostState;
use std::sync::Arc;

/// Common trait bounds for the provider type used throughout `EthPayloadValidator`
pub trait EthProvider: DatabaseProviderFactory<Provider: BlockReader + BlockNumReader + HeaderProvider>
    + BlockReader
    + BlockNumReader
    + StateProviderFactory
    + StateReader
    + StateCommitmentProvider
    + HashedPostStateProvider
    + HeaderProvider<Header = <EthPrimitives as reth_primitives_traits::NodePrimitives>::BlockHeader>
    + Clone
    + Unpin
    + 'static
{
}

/// Automatic implementation for types that satisfy the bounds
impl<P> EthProvider for P where
    P: DatabaseProviderFactory<Provider: BlockReader + BlockNumReader + HeaderProvider>
        + BlockReader
        + BlockNumReader
        + StateProviderFactory
        + StateReader
        + StateCommitmentProvider
        + HashedPostStateProvider
        + HeaderProvider<
            Header = <EthPrimitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        > + Clone
        + Unpin
        + 'static
{
}

/// Ethereum-specific payload validator that uses [`TreePayloadValidator`] for common validation
/// logic.
#[derive(Debug)]
pub struct EthPayloadValidator<P, C, Spec>
where
    P: EthProvider,
    C: ConfigureEvm<Primitives = EthPrimitives> + 'static,
{
    /// Reusable validation logic provider
    tree_validator: TreePayloadValidator<EthPrimitives, P, C>,
    /// Chain spec for hardfork checks
    chain_spec: Arc<Spec>,
}

impl<P, C, Spec> EthPayloadValidator<P, C, Spec>
where
    P: EthProvider,
    C: ConfigureEvm<Primitives = EthPrimitives> + 'static,
    Spec: EthereumHardforks,
{
    /// Creates a new Ethereum payload validator.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<EthPrimitives, Error = ConsensusError>>,
        evm_config: C,
        config: TreeConfig,
        payload_processor: PayloadProcessor<EthPrimitives, C>,
        precompile_cache_map: PrecompileCacheMap<SpecFor<C>>,
        invalid_block_hook: Box<dyn InvalidBlockHook<EthPrimitives>>,
        chain_spec: Arc<Spec>,
    ) -> Self {
        let tree_validator = TreePayloadValidator::new(
            provider,
            consensus,
            evm_config,
            config,
            payload_processor,
            precompile_cache_map,
            100, // invalid_headers_cache_size
            invalid_block_hook,
            Default::default(), // metrics
        );

        Self { tree_validator, chain_spec }
    }
}

impl<P, C, Spec> PayloadValidator for EthPayloadValidator<P, C, Spec>
where
    P: EthProvider,
    C: ConfigureEvm<Primitives = EthPrimitives> + 'static,
    Spec: EthereumHardforks + Send + Sync + 'static,
{
    type Block = EthBlock;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: Self::ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        // Use the standalone validation function with chain spec
        let sealed_block = ensure_well_formed_payload(self.chain_spec.as_ref(), payload)
            .map_err(NewPayloadError::Eth)?;

        // Recover senders for the block
        let recovered_block = sealed_block
            .try_recover()
            .map_err(|_| NewPayloadError::Other("Failed to recover senders".to_string().into()))?;

        Ok(recovered_block)
    }

    fn validate_block_post_execution_with_hashed_state(
        &self,
        _state_updates: &HashedPostState,
        _block: &RecoveredBlock<Self::Block>,
    ) -> Result<(), ConsensusError> {
        // Default implementation - no additional validation needed for Ethereum
        Ok(())
    }
}

impl<P, C, Spec> EngineValidator<EthPayloadTypes> for EthPayloadValidator<P, C, Spec>
where
    P: EthProvider,
    C: ConfigureEvm<Primitives = EthPrimitives> + 'static,
    Spec: EthereumHardforks + Send + Sync + 'static,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<
            '_,
            ExecutionData,
            <EthPayloadTypes as reth_payload_primitives::PayloadTypes>::PayloadAttributes,
        >,
    ) -> Result<(), EngineObjectValidationError> {
        // Validate execution requests for Prague (V4+)
        payload_or_attrs
            .execution_requests()
            .map(|requests| validate_execution_requests(requests))
            .transpose()?;

        // Validate version-specific fields (withdrawals, parent beacon block root, etc.)
        validate_version_specific_fields(self.chain_spec.as_ref(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &<EthPayloadTypes as reth_payload_primitives::PayloadTypes>::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        // Validate the attributes using the generic validation function
        validate_version_specific_fields(
            self.chain_spec.as_ref(),
            version,
            PayloadOrAttributes::<ExecutionData, _>::PayloadAttributes(attributes),
        )
    }

    fn validate_payload(
        &mut self,
        payload: ExecutionData,
        ctx: TreeCtx<'_, EthPrimitives>,
    ) -> Result<PayloadValidationOutcome<EthBlock>, NewPayloadError> {
        // Use the standalone validation function to convert and validate the payload
        let sealed_block = ensure_well_formed_payload(self.chain_spec.as_ref(), payload)
            .map_err(NewPayloadError::Eth)?;

        // Recover senders for the block
        let block = sealed_block
            .try_recover()
            .map_err(|_| NewPayloadError::Other("Failed to recover senders".to_string().into()))?;

        self.tree_validator.validate_block_with_state(block, ctx)
    }

    fn validate_block(
        &self,
        block: &RecoveredBlock<EthBlock>,
        ctx: TreeCtx<'_, EthPrimitives>,
    ) -> Result<(), ConsensusError> {
        self.tree_validator.validate_block(block, ctx)
    }
}
