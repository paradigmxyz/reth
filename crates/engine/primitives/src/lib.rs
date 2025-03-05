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

use alloy_consensus::BlockHeader;
use alloy_eips::eip7685::Requests;
use reth_errors::ConsensusError;
use reth_payload_primitives::{
    validate_execution_requests, BuiltPayload, EngineApiMessageVersion,
    EngineObjectValidationError, InvalidPayloadAttributesError, NewPayloadError, PayloadAttributes,
    PayloadOrAttributes, PayloadTypes,
};
use reth_primitives::{NodePrimitives, RecoveredBlock, SealedBlock};
use reth_primitives_traits::Block;
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
pub use invalid_block_hook::InvalidBlockHook;

/// This type defines the versioned types of the engine API.
///
/// This includes the execution payload types and payload attributes that are used to trigger a
/// payload job. Hence this trait is also [`PayloadTypes`].
pub trait EngineTypes:
    PayloadTypes<
        BuiltPayload: TryInto<Self::ExecutionPayloadEnvelopeV1>
                          + TryInto<Self::ExecutionPayloadEnvelopeV2>
                          + TryInto<Self::ExecutionPayloadEnvelopeV3>
                          + TryInto<Self::ExecutionPayloadEnvelopeV4>,
    > + DeserializeOwned
    + Serialize
    + 'static
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
    /// Execution data.
    type ExecutionData: ExecutionPayload;

    /// Converts a [`BuiltPayload`] into an [`Self::ExecutionData`].
    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData;
}

/// Type that validates an [`ExecutionPayload`].
#[auto_impl::auto_impl(&, Arc)]
pub trait PayloadValidator: Send + Sync + Unpin + 'static {
    /// The block type used by the engine.
    type Block: Block;

    /// The execution payload type used by the engine.
    type ExecutionData;

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
        payload: Self::ExecutionData,
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
}

/// Type that validates the payloads processed by the engine.
pub trait EngineValidator<Types: EngineTypes>:
    PayloadValidator<ExecutionData = Types::ExecutionData>
{
    /// Validates the execution requests according to [EIP-7685](https://eips.ethereum.org/EIPS/eip-7685).
    fn validate_execution_requests(
        &self,
        requests: &Requests,
    ) -> Result<(), EngineObjectValidationError> {
        validate_execution_requests(requests)
    }

    /// Validates the presence or exclusion of fork-specific fields based on the payload attributes
    /// and the message version.
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<
            '_,
            Types::ExecutionData,
            <Types as PayloadTypes>::PayloadAttributes,
        >,
    ) -> Result<(), EngineObjectValidationError>;

    /// Ensures that the payload attributes are valid for the given [`EngineApiMessageVersion`].
    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &<Types as PayloadTypes>::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError>;

    /// Validates the payload attributes with respect to the header.
    ///
    /// By default, this enforces that the payload attributes timestamp is greater than the
    /// timestamp according to:
    ///   > 7. Client software MUST ensure that payloadAttributes.timestamp is greater than
    ///   > timestamp
    ///   > of a block referenced by forkchoiceState.headBlockHash.
    ///
    /// See also [engine api spec](https://github.com/ethereum/execution-apis/tree/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine)
    fn validate_payload_attributes_against_header(
        &self,
        attr: &<Types as PayloadTypes>::PayloadAttributes,
        header: &<Self::Block as Block>::Header,
    ) -> Result<(), InvalidPayloadAttributesError> {
        if attr.timestamp() <= header.timestamp() {
            return Err(InvalidPayloadAttributesError::InvalidTimestamp);
        }
        Ok(())
    }
}
