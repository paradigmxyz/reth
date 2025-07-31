//! Traits, validation methods, and helper types used to abstract over engine types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{borrow::Cow, vec::Vec};
use alloy_consensus::BlockHeader;
use alloy_rpc_types_engine::ExecutionData;
use core::convert::Infallible;
use reth_errors::ConsensusError;
use reth_evm::{
    block::BlockExecutorFactory, eth::EthBlockExecutionCtx, execute::OwnedExecutableTxFor,
    ConfigureEvm, EvmEnvFor, ExecutionCtxFor,
};
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, InvalidPayloadAttributesError,
    NewPayloadError, PayloadAttributes, PayloadOrAttributes, PayloadTypes,
};
use reth_primitives_traits::{Block, BlockTy, RecoveredBlock, SealedBlock};
use reth_trie::iter::Either;
use reth_trie_common::HashedPostState;
use serde::{de::DeserializeOwned, Serialize};

// Re-export [`ExecutionPayload`] moved to `reth_payload_primitives`
pub use reth_payload_primitives::ExecutionPayload;

mod error;
pub use error::*;

mod forkchoice;
pub use forkchoice::{ForkchoiceStateHash, ForkchoiceStateTracker, ForkchoiceStatus};

mod message;
pub use message::*;

mod event;
pub use event::*;

mod invalid_block_hook;
pub use invalid_block_hook::{InvalidBlockHook, InvalidBlockHooks, NoopInvalidBlockHook};

pub mod config;
pub use config::*;

/// This type defines the versioned types of the engine API based on the [ethereum engine API](https://github.com/ethereum/execution-apis/tree/main/src/engine).
///
/// This includes the execution payload types and payload attributes that are used to trigger a
/// payload job. Hence this trait is also [`PayloadTypes`].
///
/// Implementations of this type are intended to be stateless and just define the types as
/// associated types.
/// This type is intended for non-ethereum chains that closely mirror the ethereum engine API spec,
/// but may have different payload, for example opstack, but structurally equivalent otherwise (same
/// engine API RPC endpoints for example).
pub trait EngineTypes:
    PayloadTypes<
        BuiltPayload: TryInto<Self::ExecutionPayloadEnvelopeV1>
                          + TryInto<Self::ExecutionPayloadEnvelopeV2>
                          + TryInto<Self::ExecutionPayloadEnvelopeV3>
                          + TryInto<Self::ExecutionPayloadEnvelopeV4>
                          + TryInto<Self::ExecutionPayloadEnvelopeV5>,
    > + DeserializeOwned
    + Serialize
{
    /// Execution Payload V1 envelope type.
    type ExecutionPayloadEnvelopeV1: DeserializeOwned
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static;
    /// Execution Payload V2  envelope type.
    type ExecutionPayloadEnvelopeV2: DeserializeOwned
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static;
    /// Execution Payload V3 envelope type.
    type ExecutionPayloadEnvelopeV3: DeserializeOwned
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static;
    /// Execution Payload V4 envelope type.
    type ExecutionPayloadEnvelopeV4: DeserializeOwned
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static;
    /// Execution Payload V5 envelope type.
    type ExecutionPayloadEnvelopeV5: DeserializeOwned
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static;
}

/// Type that validates the payloads processed by the engine.
pub trait EngineValidator<Types: PayloadTypes>: Send + Sync + Unpin + 'static {
    /// Validates the presence or exclusion of fork-specific fields based on the payload attributes
    /// and the message version.
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Types::ExecutionData, Types::PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError>;

    /// Ensures that the payload attributes are valid for the given [`EngineApiMessageVersion`].
    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &Types::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError>;
}

/// Type that validates an [`ExecutionPayload`].
#[auto_impl::auto_impl(&, Arc)]
pub trait PayloadValidator<Types: PayloadTypes>: Send + Sync + Unpin + 'static {
    /// The block type used by the engine.
    type Block: Block;

    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout.
    ///
    /// This function must convert the payload into the executable block and pre-validate its
    /// fields.
    ///
    /// Implementers should ensure that the checks are done in the order that conforms with the
    /// engine-API specification.
    fn ensure_well_formed_payload(
        &self,
        payload: Types::ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError>;

    /// Verifies payload post-execution w.r.t. hashed state updates.
    fn validate_block_post_execution_with_hashed_state(
        &self,
        _state_updates: &HashedPostState,
        _block: &RecoveredBlock<Self::Block>,
    ) -> Result<(), ConsensusError> {
        // method not used by l1
        Ok(())
    }

    /// Validates the payload attributes with respect to the header.
    ///
    /// By default, this enforces that the payload attributes timestamp is greater than the
    /// timestamp according to:
    ///   > 7. Client software MUST ensure that payloadAttributes.timestamp is greater than
    ///   > timestamp
    ///   > of a block referenced by forkchoiceState.headBlockHash.
    ///
    /// See also: <https://github.com/ethereum/execution-apis/blob/647a677b7b97e09145b8d306c0eaf51c32dae256/src/engine/common.md#specification-1>
    fn validate_payload_attributes_against_header(
        &self,
        attr: &Types::PayloadAttributes,
        header: &<Self::Block as Block>::Header,
    ) -> Result<(), InvalidPayloadAttributesError> {
        if attr.timestamp() <= header.timestamp() {
            return Err(InvalidPayloadAttributesError::InvalidTimestamp);
        }
        Ok(())
    }
}

/// An EVM-aware [`PayloadValidator`].
pub trait EvmPayloadValidator<T: PayloadTypes, Evm: ConfigureEvm>:
    PayloadValidator<T, Block = BlockTy<Evm::Primitives>>
{
    /// Returns an [`EvmEnvFor`] for the given payload.
    fn evm_env_for_payload(
        &self,
        payload: &T::ExecutionData,
        evm: &Evm,
    ) -> Result<EvmEnvFor<Evm>, NewPayloadError> {
        let block = self.ensure_well_formed_payload(payload.clone())?;
        Ok(evm.evm_env(block.header()))
    }

    /// Returns an [`ExecutableTxIterator`] for the given payload.
    fn tx_iterator_for_payload(
        &self,
        payload: &T::ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Evm>, NewPayloadError> {
        Ok(self
            .ensure_well_formed_payload(payload.clone())?
            .clone_transactions_recovered()
            .collect::<Vec<_>>()
            .into_iter()
            .map(Ok::<_, Infallible>))
    }

    /// Returns an [`ExecutionCtxProvider`] for the given payload.
    fn execution_ctx_for_payload<'a>(
        &self,
        payload: &'a T::ExecutionData,
    ) -> Result<impl ExecutionCtxProvider<Evm> + 'a, NewPayloadError> {
        Ok(self.ensure_well_formed_payload(payload.clone())?.into_sealed_block())
    }
}

/// Iterator over executable transactions.
pub trait ExecutableTxIterator<Evm: ConfigureEvm>:
    ExactSizeIterator<Item = Result<Self::Tx, Self::Error>> + Send + 'static
{
    /// The executable transaction type iterator yields.
    type Tx: OwnedExecutableTxFor<Evm>;
    /// Errors that may occur while recovering or decoding transactions.
    type Error: core::error::Error + Send + Sync + 'static;
}

impl<Evm: ConfigureEvm, Tx, Err, T> ExecutableTxIterator<Evm> for T
where
    Tx: OwnedExecutableTxFor<Evm>,
    Err: core::error::Error + Send + Sync + 'static,
    T: ExactSizeIterator<Item = Result<Tx, Err>> + Send + 'static,
{
    type Tx = Tx;
    type Error = Err;
}

/// A helper trait marking a type that can produce [`ExecutionCtxFor`] for any lifetime.
#[auto_impl::auto_impl(&)]
pub trait ExecutionCtxProvider<Evm: ConfigureEvm> {
    /// Returns an [`ExecutionCtxFor`] for the given [`ConfigureEvm`].
    fn get_ctx(&self, evm: &Evm) -> ExecutionCtxFor<'_, Evm>;
}

impl<Evm: ConfigureEvm> ExecutionCtxProvider<Evm> for SealedBlock<BlockTy<Evm::Primitives>> {
    /// Returns an [`ExecutionCtxFor`] for the given [`ConfigureEvm`].
    fn get_ctx(&self, evm: &Evm) -> ExecutionCtxFor<'_, Evm> {
        evm.context_for_block(self)
    }
}

impl<L, R, Evm> ExecutionCtxProvider<Evm> for Either<L, R>
where
    Evm: ConfigureEvm,
    L: ExecutionCtxProvider<Evm>,
    R: ExecutionCtxProvider<Evm>,
{
    fn get_ctx(&self, evm: &Evm) -> ExecutionCtxFor<'_, Evm> {
        match self {
            Self::Left(l) => l.get_ctx(evm),
            Self::Right(r) => r.get_ctx(evm),
        }
    }
}

impl<Evm> ExecutionCtxProvider<Evm> for ExecutionData
where
    Evm: ConfigureEvm<
        BlockExecutorFactory: for<'a> BlockExecutorFactory<
            ExecutionCtx<'a> = EthBlockExecutionCtx<'a>,
        >,
    >,
{
    fn get_ctx(&self, _evm: &Evm) -> ExecutionCtxFor<'_, Evm> {
        EthBlockExecutionCtx {
            parent_hash: self.parent_hash(),
            parent_beacon_block_root: self.sidecar.parent_beacon_block_root(),
            ommers: &[],
            withdrawals: self.payload.as_v2().map(|v2| Cow::Owned(v2.withdrawals.clone().into())),
        }
    }
}

#[cfg(feature = "op")]
impl<Evm> ExecutionCtxProvider<Evm> for op_alloy_rpc_types_engine::OpExecutionData
where
    Evm: ConfigureEvm<
        BlockExecutorFactory: for<'a> BlockExecutorFactory<
            ExecutionCtx<'a> = alloy_op_evm::OpBlockExecutionCtx,
        >,
    >,
{
    fn get_ctx(&self, _evm: &Evm) -> ExecutionCtxFor<'_, Evm> {
        alloy_op_evm::OpBlockExecutionCtx {
            parent_hash: self.parent_hash(),
            parent_beacon_block_root: self.sidecar.parent_beacon_block_root(),
            extra_data: self.payload.as_v1().extra_data.clone(),
        }
    }
}
