//! API for block submission validation.

use alloy_primitives::B256;
use alloy_rpc_types_beacon::relay::{
    BuilderBlockValidationRequest, BuilderBlockValidationRequestV2, SignedBidSubmissionV3,
    SignedBidSubmissionV4,
};
use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

/// A Request to validate a [`SignedBidSubmissionV3`]
///
/// <https://github.com/flashbots/builder/blob/7577ac81da21e760ec6693637ce2a81fe58ac9f8/eth/block-validation/api.go#L198-L202>
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderBlockValidationRequestV3 {
    /// The request to be validated.
    #[serde(flatten)]
    pub request: SignedBidSubmissionV3,
    /// The registered gas limit for the validation request.
    #[serde_as(as = "DisplayFromStr")]
    pub registered_gas_limit: u64,
    /// The parent beacon block root for the validation request.
    pub parent_beacon_block_root: B256,
}

/// A Request to validate a [`SignedBidSubmissionV4`]
///
/// <https://github.com/flashbots/builder/blob/7577ac81da21e760ec6693637ce2a81fe58ac9f8/eth/block-validation/api.go#L198-L202>
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderBlockValidationRequestV4 {
    /// The request to be validated.
    #[serde(flatten)]
    pub request: SignedBidSubmissionV4,
    /// The registered gas limit for the validation request.
    #[serde_as(as = "DisplayFromStr")]
    pub registered_gas_limit: u64,
    /// The parent beacon block root for the validation request.
    pub parent_beacon_block_root: B256,
}

/// Block validation rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "flashbots"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "flashbots"))]
pub trait BlockSubmissionValidationApi {
    /// A Request to validate a block submission.
    #[method(name = "validateBuilderSubmissionV1")]
    async fn validate_builder_submission_v1(
        &self,
        request: BuilderBlockValidationRequest,
    ) -> jsonrpsee::core::RpcResult<()>;

    /// A Request to validate a block submission.
    #[method(name = "validateBuilderSubmissionV2")]
    async fn validate_builder_submission_v2(
        &self,
        request: BuilderBlockValidationRequestV2,
    ) -> jsonrpsee::core::RpcResult<()>;

    /// A Request to validate a block submission.
    #[method(name = "validateBuilderSubmissionV3")]
    async fn validate_builder_submission_v3(
        &self,
        request: BuilderBlockValidationRequestV3,
    ) -> jsonrpsee::core::RpcResult<()>;

    /// A Request to validate a block submission.
    #[method(name = "validateBuilderSubmissionV4")]
    async fn validate_builder_submission_v4(
        &self,
        request: BuilderBlockValidationRequestV4,
    ) -> jsonrpsee::core::RpcResult<()>;
}
