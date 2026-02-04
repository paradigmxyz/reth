//! Reth-specific engine API extensions.
//!
//! This module provides reth-specific engine API endpoints that mirror the standard
//! `engine_` namespace but under the `reth_` namespace.

use alloy_eips::eip7685::RequestsOrHash;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{
    ExecutionPayloadInputV2, ExecutionPayloadV1, ExecutionPayloadV3, PayloadStatus,
};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Reth-specific engine API extensions.
///
/// This trait provides `reth_newPayload*` endpoints that mirror the standard
/// `engine_newPayload*` endpoints but are available under the `reth_` namespace.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "reth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "reth"))]
pub trait RethEngineApi {
    /// Reth-specific version of `engine_newPayloadV1`.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "newPayloadV1")]
    async fn reth_new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus>;

    /// Reth-specific version of `engine_newPayloadV2`.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    #[method(name = "newPayloadV2")]
    async fn reth_new_payload_v2(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> RpcResult<PayloadStatus>;

    /// Reth-specific version of `engine_newPayloadV3`.
    ///
    /// Post Cancun payload handler.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_newpayloadv3>
    #[method(name = "newPayloadV3")]
    async fn reth_new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;

    /// Reth-specific version of `engine_newPayloadV4`.
    ///
    /// Post Prague payload handler.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_newpayloadv4>
    #[method(name = "newPayloadV4")]
    async fn reth_new_payload_v4(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: RequestsOrHash,
    ) -> RpcResult<PayloadStatus>;
}
