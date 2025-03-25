//! API for block submission validation.

use alloy_primitives::B256;
use alloy_rpc_types_beacon::relay::{
    BuilderBlockValidationRequest, BuilderBlockValidationRequestV2, SignedBidSubmissionV3,
    SignedBidSubmissionV4,
};
use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};

// Ultra Sound custom types to support per request transaction filtering

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum TransactionFilter {
    #[default]
    None,
    OFAC,
}

#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct BuilderBlockValidationRequestV3 {
    /// The request to be validated.
    #[serde(flatten)]
    pub request: SignedBidSubmissionV3,
    /// The registered gas limit for the validation request.
    #[serde_as(as = "DisplayFromStr")]
    pub registered_gas_limit: u64,
    /// The parent beacon block root for the validation request.
    pub parent_beacon_block_root: B256,
    pub transaction_filter: TransactionFilter,
}

/// A Request to validate a [`SignedBidSubmissionV4`]
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct BuilderBlockValidationRequestV4 {
    /// The request to be validated.
    #[serde(flatten)]
    pub request: SignedBidSubmissionV4,
    /// The registered gas limit for the validation request.
    #[serde_as(as = "DisplayFromStr")]
    pub registered_gas_limit: u64,
    /// The parent beacon block root for the validation request.
    pub parent_beacon_block_root: B256,
    #[serde(default)]
    pub transaction_filter: TransactionFilter,
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
