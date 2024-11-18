use crate::traits::PayloadEnvelopeExt;
use alloy_primitives::B256;
use jsonrpsee::{
    core::client::ClientT,
    http_client::{transport::HttpBackend, HttpClient},
};
use reth::{
    api::{EngineTypes, PayloadBuilderAttributes},
    providers::CanonStateNotificationStream,
    rpc::{
        api::EngineApiClient,
        types::engine::{ForkchoiceState, PayloadStatusEnum},
    },
};
use reth_chainspec::EthereumHardforks;
use reth_node_builder::BuiltPayload;
use reth_payload_builder::PayloadId;
use reth_rpc_layer::AuthClientService;
use std::{marker::PhantomData, sync::Arc};

/// Helper for engine api operations
#[derive(Debug)]
pub struct EngineApiTestContext<E, ChainSpec> {
    pub chain_spec: Arc<ChainSpec>,
    pub canonical_stream: CanonStateNotificationStream,
    pub engine_api_client: HttpClient<AuthClientService<HttpBackend>>,
    pub _marker: PhantomData<E>,
}

impl<E: EngineTypes, ChainSpec: EthereumHardforks> EngineApiTestContext<E, ChainSpec> {
    /// Retrieves a v3 payload from the engine api
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<E::ExecutionPayloadEnvelopeV3> {
        Ok(EngineApiClient::<E>::get_payload_v3(&self.engine_api_client, payload_id).await?)
    }

    /// Retrieves a v3 payload from the engine api as serde value
    pub async fn get_payload_v3_value(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<serde_json::Value> {
        Ok(self.engine_api_client.request("engine_getPayloadV3", (payload_id,)).await?)
    }

    /// Submits a payload to the engine api
    pub async fn submit_payload(
        &self,
        payload: E::BuiltPayload,
        payload_builder_attributes: E::PayloadBuilderAttributes,
        expected_status: PayloadStatusEnum,
    ) -> eyre::Result<B256>
    where
        E::ExecutionPayloadEnvelopeV3: From<E::BuiltPayload> + PayloadEnvelopeExt,
        E::ExecutionPayloadEnvelopeV4: From<E::BuiltPayload> + PayloadEnvelopeExt,
    {
        let versioned_hashes =
            payload.block().blob_versioned_hashes_iter().copied().collect::<Vec<_>>();
        // submit payload to engine api
        let submission = if self
            .chain_spec
            .is_prague_active_at_timestamp(payload_builder_attributes.timestamp())
        {
            let requests = payload.requests().unwrap();
            let envelope: <E as EngineTypes>::ExecutionPayloadEnvelopeV4 = payload.into();
            EngineApiClient::<E>::new_payload_v4(
                &self.engine_api_client,
                envelope.execution_payload(),
                versioned_hashes,
                payload_builder_attributes.parent_beacon_block_root().unwrap(),
                requests,
            )
            .await?
        } else {
            let envelope: <E as EngineTypes>::ExecutionPayloadEnvelopeV3 = payload.into();
            EngineApiClient::<E>::new_payload_v3(
                &self.engine_api_client,
                envelope.execution_payload(),
                versioned_hashes,
                payload_builder_attributes.parent_beacon_block_root().unwrap(),
            )
            .await?
        };

        assert_eq!(submission.status, expected_status);

        Ok(submission.latest_valid_hash.unwrap_or_default())
    }

    /// Sends forkchoice update to the engine api
    pub async fn update_forkchoice(&self, current_head: B256, new_head: B256) -> eyre::Result<()> {
        EngineApiClient::<E>::fork_choice_updated_v2(
            &self.engine_api_client,
            ForkchoiceState {
                head_block_hash: new_head,
                safe_block_hash: current_head,
                finalized_block_hash: current_head,
            },
            None,
        )
        .await?;
        Ok(())
    }

    /// Sends forkchoice update to the engine api with a zero finalized hash
    pub async fn update_optimistic_forkchoice(&self, hash: B256) -> eyre::Result<()> {
        EngineApiClient::<E>::fork_choice_updated_v2(
            &self.engine_api_client,
            ForkchoiceState {
                head_block_hash: hash,
                safe_block_hash: B256::ZERO,
                finalized_block_hash: B256::ZERO,
            },
            None,
        )
        .await?;

        Ok(())
    }
}
