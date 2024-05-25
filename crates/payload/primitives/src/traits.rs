use crate::{
    error::{EngineObjectValidationError, PayloadBuilderError},
    validate_version_specific_fields,
};
use reth_primitives::{
    alloy_primitives::private::serde,
    revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg},
    Address, ChainSpec, Header, SealedBlock, Withdrawals, B256, U256,
};
use reth_provider::CanonStateNotification;
use reth_rpc_types::{
    engine::{OptimismPayloadAttributes, PayloadAttributes as EthPayloadAttributes, PayloadId},
    Withdrawal,
};
use std::future::Future;

/// Represents a built payload type that contains a built [SealedBlock] and can be converted into
/// engine API execution payloads.
pub trait BuiltPayload: Send + Sync + std::fmt::Debug {
    /// Returns the built block (sealed)
    fn block(&self) -> &SealedBlock;

    /// Returns the fees collected for the built block
    fn fees(&self) -> U256;
}

/// This can be implemented by types that describe a currently running payload job.
///
/// This is used as a conversion type, transforming a payload attributes type that the engine API
/// receives, into a type that the payload builder can use.
pub trait PayloadBuilderAttributes: Send + Sync + std::fmt::Debug {
    /// The payload attributes that can be used to construct this type. Used as the argument in
    /// [PayloadBuilderAttributes::try_new].
    type RpcPayloadAttributes;
    /// The error type used in [PayloadBuilderAttributes::try_new].
    type Error: std::error::Error;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    fn try_new(
        parent: B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;

    /// Returns the [PayloadId] for the running payload job.
    fn payload_id(&self) -> PayloadId;

    /// Returns the parent block hash for the running payload job.
    fn parent(&self) -> B256;

    /// Returns the timestamp for the running payload job.
    fn timestamp(&self) -> u64;

    /// Returns the parent beacon block root for the running payload job, if it exists.
    fn parent_beacon_block_root(&self) -> Option<B256>;

    /// Returns the suggested fee recipient for the running payload job.
    fn suggested_fee_recipient(&self) -> Address;

    /// Returns the prevrandao field for the running payload job.
    fn prev_randao(&self) -> B256;

    /// Returns the withdrawals for the running payload job.
    fn withdrawals(&self) -> &Withdrawals;

    /// Returns the configured [CfgEnvWithHandlerCfg] and [BlockEnv] for the targeted payload (that
    /// has the `parent` as its parent).
    ///
    /// The `chain_spec` is used to determine the correct chain id and hardfork for the payload
    /// based on its timestamp.
    ///
    /// Block related settings are derived from the `parent` block and the configured attributes.
    ///
    /// NOTE: This is only intended for beacon consensus (after merge).
    fn cfg_and_block_env(
        &self,
        chain_spec: &ChainSpec,
        parent: &Header,
    ) -> (CfgEnvWithHandlerCfg, BlockEnv);
}

/// A type that can build a payload.
///
/// This type is a [`Future`] that resolves when the job is done (e.g. complete, timed out) or it
/// failed. It's not supposed to return the best payload built when it resolves, instead
/// [`PayloadJob::best_payload`] should be used for that.
///
/// A `PayloadJob` must always be prepared to return the best payload built so far to ensure there
/// is a valid payload to deliver to the CL, so it does not miss a slot, even if the payload is
/// empty.
///
/// Note: A `PayloadJob` need to be cancel safe because it might be dropped after the CL has requested the payload via `engine_getPayloadV1` (see also [engine API docs](https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_getpayloadv1))
pub trait PayloadJob: Future<Output = Result<(), PayloadBuilderError>> + Send + Sync {
    /// Represents the payload attributes type that is used to spawn this payload job.
    type PayloadAttributes: PayloadBuilderAttributes + std::fmt::Debug;
    /// Represents the future that resolves the block that's returned to the CL.
    type ResolvePayloadFuture: Future<Output = Result<Self::BuiltPayload, PayloadBuilderError>>
        + Send
        + Sync
        + 'static;
    /// Represents the built payload type that is returned to the CL.
    type BuiltPayload: BuiltPayload + Clone + std::fmt::Debug;

    /// Returns the best payload that has been built so far.
    ///
    /// Note: This is never called by the CL.
    fn best_payload(&self) -> Result<Self::BuiltPayload, PayloadBuilderError>;

    /// Returns the payload attributes for the payload being built.
    fn payload_attributes(&self) -> Result<Self::PayloadAttributes, PayloadBuilderError>;

    /// Called when the payload is requested by the CL.
    ///
    /// This is invoked on [`engine_getPayloadV2`](https://github.com/ethereum/execution-apis/blob/main/src/engine/shanghai.md#engine_getpayloadv2) and [`engine_getPayloadV1`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#engine_getpayloadv1).
    ///
    /// The timeout for returning the payload to the CL is 1s, thus the future returned should
    /// resolve in under 1 second.
    ///
    /// Ideally this is the best payload built so far, or an empty block without transactions, if
    /// nothing has been built yet.
    ///
    /// According to the spec:
    /// > Client software MAY stop the corresponding build process after serving this call.
    ///
    /// It is at the discretion of the implementer whether the build job should be kept alive or
    /// terminated.
    ///
    /// If this returns [`KeepPayloadJobAlive::Yes`], then the [`PayloadJob`] will be polled
    /// once more. If this returns [`KeepPayloadJobAlive::No`] then the [`PayloadJob`] will be
    /// dropped after this call.
    fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive);
}

/// Whether the payload job should be kept alive or terminated after the payload was requested by
/// the CL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepPayloadJobAlive {
    /// Keep the job alive.
    Yes,
    /// Terminate the job.
    No,
}

/// A type that knows how to create new jobs for creating payloads.
pub trait PayloadJobGenerator: Send + Sync {
    /// The type that manages the lifecycle of a payload.
    ///
    /// This type is a future that yields better payloads.
    type Job: PayloadJob;

    /// Creates the initial payload and a new [`PayloadJob`] that yields better payloads over time.
    ///
    /// This is called when the CL requests a new payload job via a fork choice update.
    ///
    /// # Note
    ///
    /// This is expected to initially build a new (empty) payload without transactions, so it can be
    /// returned directly.
    fn new_payload_job(
        &self,
        attr: <Self::Job as PayloadJob>::PayloadAttributes,
    ) -> Result<Self::Job, PayloadBuilderError>;

    /// Handles new chain state events
    ///
    /// This is intended for any logic that needs to be run when the chain state changes or used to
    /// use the in memory state for the head block.
    fn on_new_state(&mut self, new_state: CanonStateNotification) {
        let _ = new_state;
    }
}

// TODO: Consider to move this elsewhere
/// The version of Engine API message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineApiMessageVersion {
    /// Version 1
    V1,
    /// Version 2
    ///
    /// Added in the Shanghai hardfork.
    V2,
    /// Version 3
    ///
    /// Added in the Cancun hardfork.
    V3,
    /// Version 4
    ///
    /// Added in the Prague hardfork.
    V4,
}

/// The execution payload attribute type the CL node emits via the engine API.
/// This trait should be implemented by types that could be used to spawn a payload job.
///
/// This type is emitted as part of the forkchoiceUpdated call
pub trait PayloadAttributes:
    serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug + Clone + Send + Sync + 'static
{
    /// Returns the timestamp to be used in the payload job.
    fn timestamp(&self) -> u64;

    /// Returns the withdrawals for the given payload attributes.
    fn withdrawals(&self) -> Option<&Vec<Withdrawal>>;

    /// Return the parent beacon block root for the payload attributes.
    fn parent_beacon_block_root(&self) -> Option<B256>;

    /// Ensures that the payload attributes are valid for the given [ChainSpec] and
    /// [EngineApiMessageVersion].
    fn ensure_well_formed_attributes(
        &self,
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
    ) -> Result<(), EngineObjectValidationError>;
}

impl PayloadAttributes for EthPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.parent_beacon_block_root
    }

    fn ensure_well_formed_attributes(
        &self,
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(chain_spec, version, self.into())
    }
}

impl PayloadAttributes for OptimismPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.payload_attributes.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.payload_attributes.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.payload_attributes.parent_beacon_block_root
    }

    fn ensure_well_formed_attributes(
        &self,
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(chain_spec, version, self.into())?;

        if self.gas_limit.is_none() && chain_spec.is_optimism() {
            return Err(EngineObjectValidationError::InvalidParams(
                "MissingGasLimitInPayloadAttributes".to_string().into(),
            ))
        }

        Ok(())
    }
}
