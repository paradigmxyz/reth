//! Traits, validation methods, and helper types used to abstract over engine types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloy_consensus::BlockHeader;
use reth_errors::ConsensusError;
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, InvalidPayloadAttributesError,
    NewPayloadError, PayloadAttributes, PayloadOrAttributes, PayloadTypes,
};
use reth_primitives_traits::{Block, RecoveredBlock, SealedBlock};
use reth_trie_common::HashedPostState;
use serde::{de::DeserializeOwned, Serialize};

// Re-export [`ExecutionPayload`] moved to `reth_payload_primitives`
#[cfg(feature = "std")]
pub use reth_evm::{ConfigureEngineEvm, ConvertTx, ExecutableTxIterator, ExecutableTxTuple};
pub use reth_payload_primitives::ExecutionPayload;

mod error;
pub use error::*;

mod forkchoice;
pub use forkchoice::{ForkchoiceStateHash, ForkchoiceStateTracker, ForkchoiceStatus};

#[cfg(feature = "std")]
mod message;
#[cfg(feature = "std")]
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
                          + TryInto<Self::ExecutionPayloadEnvelopeV5>
                          + TryInto<Self::ExecutionPayloadEnvelopeV6>,
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
    /// Execution Payload V6 envelope type.
    type ExecutionPayloadEnvelopeV6: DeserializeOwned
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static;
}

/// Validates engine API requests at the RPC layer, before payloads and attributes
/// are forwarded to the engine for processing.
///
/// - [`validate_version_specific_fields`](Self::validate_version_specific_fields): Enforced in each
///   `engine_newPayloadVN` RPC handler to verify the payload contains the correct fields for the
///   engine API version (e.g., blob fields in V3+, requests in V4+).
///
/// - [`ensure_well_formed_attributes`](Self::ensure_well_formed_attributes): Enforced in
///   `engine_forkchoiceUpdatedVN` RPC handlers to validate payload attributes are well-formed for
///   the given version before forwarding to the engine.
///
/// After this validation passes, the engine performs the full consensus validation
/// pipeline (header, pre-execution, execution, post-execution).
pub trait EngineApiValidator<Types: PayloadTypes>: Send + Sync + Unpin + 'static {
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
///
/// This trait handles validation at the engine API boundary — converting payloads
/// into blocks and validating payload attributes for block building.
///
/// # Methods and when they're used
///
/// - [`convert_payload_to_block`](Self::convert_payload_to_block): Used during `engine_newPayload`
///   processing to decode the payload into a [`SealedBlock`]. Also used to validate payload
///   structure during backfill buffering. In the engine tree, this runs concurrently with state
///   setup on a background thread.
///
/// - [`ensure_well_formed_payload`](Self::ensure_well_formed_payload): Converts payload and
///   recovers transaction signatures. Used when recovered senders are needed immediately.
///
/// - [`validate_payload_attributes_against_header`](Self::validate_payload_attributes_against_header):
///   Enforced as part of the engine's `forkchoiceUpdated` handling when payload attributes
///   are provided. Gates whether a payload build job is started.
///
/// - [`validate_block_post_execution_with_hashed_state`](Self::validate_block_post_execution_with_hashed_state):
///   Called after block execution in the engine's payload validation pipeline.
///   No-op on L1, used by L2s (e.g., OP Stack) for additional post-execution checks.
///
/// # Relationship to consensus traits
///
/// This trait does NOT replace the consensus traits (`Consensus`, `FullConsensus` from
/// `reth-consensus`). Those handle the actual consensus rule
/// validation (header checks, pre/post-execution). This trait handles engine API-specific
/// concerns: payload encoding/decoding and attribute validation.
#[auto_impl::auto_impl(&, Arc)]
pub trait PayloadValidator<Types: PayloadTypes>: Send + Sync + Unpin + 'static {
    /// The block type used by the engine.
    type Block: Block;

    /// Converts the given payload into a sealed block without recovering signatures.
    ///
    /// This function validates the payload and converts it into a [`SealedBlock`] which contains
    /// the block hash but does not perform signature recovery on transactions.
    ///
    /// This is more efficient than [`Self::ensure_well_formed_payload`] when signature recovery
    /// is not needed immediately or will be performed later.
    ///
    /// Implementers should ensure that the checks are done in the order that conforms with the
    /// engine-API specification.
    fn convert_payload_to_block(
        &self,
        payload: Types::ExecutionData,
    ) -> Result<SealedBlock<Self::Block>, NewPayloadError>;

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
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let sealed_block = self.convert_payload_to_block(payload)?;
        sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
    }

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
    /// See also: <https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#specification-1>
    ///
    /// Enforced as part of the engine's `forkchoiceUpdated` handling when the consensus layer
    /// provides payload attributes. If this returns an error, the forkchoice state update itself
    /// is NOT rolled back, but no payload build job is started — the response includes
    /// `INVALID_PAYLOAD_ATTRIBUTES`.
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
