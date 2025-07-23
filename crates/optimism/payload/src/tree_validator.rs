//! Optimism-specific tree payload validator implementation.

use op_alloy_rpc_types_engine::OpExecutionData;
use reth_chainspec::EthereumHardforks;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_engine_primitives::{InvalidBlockHook, PayloadValidator};
use reth_engine_tree::tree::{
    payload_validator::{PayloadValidationOutcome, TreeCtx},
    precompile_cache::PrecompileCacheMap,
    EngineValidator, PayloadProcessor, TreeConfig, TreePayloadValidator,
};
use reth_evm::{ConfigureEvm, SpecFor};
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::{OpBlock, OpPrimitives};
use reth_payload_primitives::{
    validate_version_specific_fields, EngineApiMessageVersion, EngineObjectValidationError,
    NewPayloadError, PayloadOrAttributes,
};
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{
    BlockNumReader, BlockReader, DatabaseProviderFactory, HashedPostStateProvider, HeaderProvider,
    StateCommitmentProvider, StateProviderFactory, StateReader,
};
use reth_trie::HashedPostState;
use std::sync::Arc;

// Import the standalone validation function
use crate::{validator::ensure_well_formed_payload, OpPayloadTypes};

/// Common trait bounds for the provider type used throughout [`OpPayloadValidator`]
pub trait OpProvider:
    DatabaseProviderFactory<Provider: BlockReader + BlockNumReader + HeaderProvider>
    + BlockReader
    + BlockNumReader
    + StateProviderFactory
    + StateReader
    + StateCommitmentProvider
    + HashedPostStateProvider
    + HeaderProvider<Header = <OpPrimitives as reth_primitives_traits::NodePrimitives>::BlockHeader>
    + Clone
    + Unpin
    + 'static
{
}

/// Automatic implementation for types that satisfy the bounds
impl<P> OpProvider for P where
    P: DatabaseProviderFactory<Provider: BlockReader + BlockNumReader + HeaderProvider>
        + BlockReader
        + BlockNumReader
        + StateProviderFactory
        + StateReader
        + StateCommitmentProvider
        + HashedPostStateProvider
        + HeaderProvider<
            Header = <OpPrimitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
        > + Clone
        + Unpin
        + 'static
{
}

/// Optimism-specific payload validator that uses [`TreePayloadValidator`] for common validation
/// logic.
#[derive(Debug)]
pub struct OpPayloadValidator<P, C, Spec>
where
    P: OpProvider,
    C: ConfigureEvm<Primitives = OpPrimitives> + 'static,
{
    /// Reusable validation logic provider
    tree_validator: TreePayloadValidator<OpPrimitives, P, C>,
    /// Chain spec for hardfork checks
    chain_spec: Arc<Spec>,
}

impl<P, C, Spec> OpPayloadValidator<P, C, Spec>
where
    P: OpProvider,
    C: ConfigureEvm<Primitives = OpPrimitives> + 'static,
    Spec: OpHardforks,
{
    /// Creates a new Optimism payload validator.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<OpPrimitives, Error = ConsensusError>>,
        evm_config: C,
        config: TreeConfig,
        payload_processor: PayloadProcessor<OpPrimitives, C>,
        precompile_cache_map: PrecompileCacheMap<SpecFor<C>>,
        invalid_block_hook: Box<dyn InvalidBlockHook<OpPrimitives>>,
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

impl<P, C, Spec> PayloadValidator for OpPayloadValidator<P, C, Spec>
where
    P: OpProvider,
    C: ConfigureEvm<Primitives = OpPrimitives> + 'static,
    Spec: OpHardforks + EthereumHardforks + Send + Sync + 'static,
{
    type Block = OpBlock;
    type ExecutionData = OpExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: Self::ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        // Use the standalone validation function with chain spec
        let sealed_block = ensure_well_formed_payload(self.chain_spec.as_ref(), payload)
            .map_err(|e| NewPayloadError::Other(e.to_string().into()))?;

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
        // Default implementation - no additional validation needed for Optimism
        Ok(())
    }
}

impl<P, C, Spec> EngineValidator<OpPayloadTypes> for OpPayloadValidator<P, C, Spec>
where
    P: OpProvider,
    C: ConfigureEvm<Primitives = OpPrimitives> + 'static,
    Spec: OpHardforks + EthereumHardforks + Send + Sync + 'static,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<
            '_,
            OpExecutionData,
            <OpPayloadTypes as reth_payload_primitives::PayloadTypes>::PayloadAttributes,
        >,
    ) -> Result<(), EngineObjectValidationError> {
        // For Optimism, execution requests should always be empty in V4+
        // The sidecar contains isthmus fields which should have empty execution requests

        // Validate version-specific fields (withdrawals, parent beacon block root, etc.)
        validate_version_specific_fields(self.chain_spec.as_ref(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &<OpPayloadTypes as reth_payload_primitives::PayloadTypes>::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        // Validate the attributes using the generic validation function
        validate_version_specific_fields(
            self.chain_spec.as_ref(),
            version,
            PayloadOrAttributes::<OpExecutionData, _>::PayloadAttributes(attributes),
        )
    }

    fn validate_payload(
        &mut self,
        payload: OpExecutionData,
        ctx: TreeCtx<'_, OpPrimitives>,
    ) -> Result<PayloadValidationOutcome<OpBlock>, NewPayloadError> {
        // Use the standalone validation function to convert and validate the payload
        let sealed_block = ensure_well_formed_payload(self.chain_spec.as_ref(), payload)
            .map_err(|e| NewPayloadError::Other(e.to_string().into()))?;

        // Recover senders for the block
        let block = sealed_block
            .try_recover()
            .map_err(|_| NewPayloadError::Other("Failed to recover senders".to_string().into()))?;

        self.tree_validator.validate_block_with_state(block, ctx)
    }

    fn validate_block(
        &self,
        block: &RecoveredBlock<OpBlock>,
        ctx: TreeCtx<'_, OpPrimitives>,
    ) -> Result<(), ConsensusError> {
        self.tree_validator.validate_block(block, ctx)
    }
}
