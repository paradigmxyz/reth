use super::{ScrollEngineApi, ScrollEngineApiResult};
use alloy_primitives::{BlockHash, U64};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ExecutionPayloadV1, ForkchoiceState,
    ForkchoiceUpdated, PayloadId, PayloadStatus,
};

use reth_rpc_api::EngineApiClient;
use reth_scroll_engine_primitives::ScrollEngineTypes;
use scroll_alloy_rpc_types_engine::ScrollPayloadAttributes;

/// A Client for a type that implements the [`EngineApiClient`] trait.
#[derive(Debug)]
pub struct ScrollAuthApiEngineClient<T> {
    client: T,
}

impl<T> ScrollAuthApiEngineClient<T> {
    /// Creates a new [`ScrollAuthApiEngineClient`] with the given client.
    pub const fn new(client: T) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<EC: EngineApiClient<ScrollEngineTypes> + Sync> ScrollEngineApi
    for ScrollAuthApiEngineClient<EC>
{
    async fn new_payload_v1(
        &self,
        payload: ExecutionPayloadV1,
    ) -> ScrollEngineApiResult<PayloadStatus> {
        Ok(self.client.new_payload_v1(payload).await?)
    }

    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<ScrollPayloadAttributes>,
    ) -> ScrollEngineApiResult<ForkchoiceUpdated> {
        Ok(self.client.fork_choice_updated_v1(fork_choice_state, payload_attributes).await?)
    }

    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> ScrollEngineApiResult<ExecutionPayloadV1> {
        Ok(self.client.get_payload_v1(payload_id).await?)
    }

    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> ScrollEngineApiResult<ExecutionPayloadBodiesV1> {
        Ok(self.client.get_payload_bodies_by_hash_v1(block_hashes).await?)
    }

    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> ScrollEngineApiResult<ExecutionPayloadBodiesV1> {
        Ok(self.client.get_payload_bodies_by_range_v1(start, count).await?)
    }

    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> ScrollEngineApiResult<Vec<ClientVersionV1>> {
        Ok(self.client.get_client_version_v1(client_version).await?)
    }

    async fn exchange_capabilities(
        &self,
        capabilities: Vec<String>,
    ) -> ScrollEngineApiResult<Vec<String>> {
        Ok(self.exchange_capabilities(capabilities).await?)
    }
}
