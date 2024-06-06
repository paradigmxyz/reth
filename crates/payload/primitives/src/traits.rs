use reth_primitives::{
    Address,
    B256, ChainSpec, Header, revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg}, SealedBlock, U256, Withdrawals,
};
use reth_rpc_types::engine::PayloadId;

/// Represents a built payload type that contains a built [`SealedBlock`] and can be converted into
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
    /// [`PayloadBuilderAttributes::try_new`].
    type RpcPayloadAttributes;
    /// The error type used in [`PayloadBuilderAttributes::try_new`].
    type Error: std::error::Error;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [`PayloadId`] for the given parent and attributes
    fn try_new(
        parent: B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
    ) -> Result<Self, Self::Error>
        where
            Self: Sized;

    /// Returns the [`PayloadId`] for the running payload job.
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

    /// Returns the configured [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the targeted payload
    /// (that has the `parent` as its parent).
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
