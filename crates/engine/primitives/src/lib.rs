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
use alloy_eips::BlockNumHash;
use reth_chain_state::CanonicalInMemoryState;
use reth_errors::ConsensusError;
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, InvalidPayloadAttributesError,
    NewPayloadError, PayloadAttributes, PayloadOrAttributes, PayloadTypes,
};
use reth_primitives_traits::{Block, NodePrimitives, RecoveredBlock};
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

pub mod config;
pub use config::*;

/// Outcome of validating a payload
#[derive(Debug)]
pub enum PayloadValidationOutcome<Block: reth_primitives_traits::Block> {
    /// Payload is valid and produced a block
    Valid {
        /// The block created from the payload
        block: RecoveredBlock<Block>,
        /// The trie updates from state root computation
        trie_updates: reth_trie::updates::TrieUpdates,
        /// Whether the block is a fork
        is_fork: bool,
    },
    /// Payload is invalid but block construction succeeded
    Invalid {
        /// The block created from the payload
        block: RecoveredBlock<Block>,
        /// The validation error
        error: NewPayloadError,
    },
}

/// Information about the current persistence state for validation context
#[derive(Debug, Clone, Copy)]
pub struct PersistenceInfo {
    /// The last persisted block
    pub last_persisted_block: BlockNumHash,
    /// The current persistence action, if any
    pub current_action: Option<PersistenceAction>,
}

impl PersistenceInfo {
    /// Creates a new persistence info with no current action
    pub const fn new(last_persisted_block: BlockNumHash) -> Self {
        Self { last_persisted_block, current_action: None }
    }

    /// Creates persistence info with a saving blocks action
    pub const fn with_saving_blocks(
        last_persisted_block: BlockNumHash,
        highest: BlockNumHash,
    ) -> Self {
        Self {
            last_persisted_block,
            current_action: Some(PersistenceAction::SavingBlocks { highest }),
        }
    }

    /// Creates persistence info with a removing blocks action
    pub const fn with_removing_blocks(
        last_persisted_block: BlockNumHash,
        new_tip_num: u64,
    ) -> Self {
        Self {
            last_persisted_block,
            current_action: Some(PersistenceAction::RemovingBlocks { new_tip_num }),
        }
    }
}

/// The type of persistence action currently in progress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceAction {
    /// Saving blocks to disk
    SavingBlocks {
        /// The highest block being saved
        highest: BlockNumHash,
    },
    /// Removing blocks from disk
    RemovingBlocks {
        /// The new tip after removal
        new_tip_num: u64,
    },
}

/// Context providing access to tree state during validation
#[derive(Debug)]
pub struct TreeCtx<'a, State, N: NodePrimitives = reth_ethereum_primitives::EthPrimitives> {
    /// The engine API tree state
    state: &'a State,
    /// Information about the current persistence state
    persistence_info: PersistenceInfo,
    /// Reference to the canonical in-memory state
    canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
}

impl<'a, State, N: NodePrimitives> TreeCtx<'a, State, N> {
    /// Creates a new tree context
    pub const fn new(
        state: &'a State,
        persistence_info: PersistenceInfo,
        canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
    ) -> Self {
        Self { state, persistence_info, canonical_in_memory_state }
    }

    /// Returns a reference to the engine API tree state
    pub const fn state(&self) -> &'a State {
        self.state
    }

    /// Returns a reference to the persistence info
    pub const fn persistence_info(&self) -> &PersistenceInfo {
        &self.persistence_info
    }

    /// Returns a reference to the canonical in-memory state
    pub const fn canonical_in_memory_state(&self) -> &'a CanonicalInMemoryState<N> {
        self.canonical_in_memory_state
    }
}

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
pub trait EngineValidator<Types: PayloadTypes>:
    PayloadValidator<ExecutionData = Types::ExecutionData>
{
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
