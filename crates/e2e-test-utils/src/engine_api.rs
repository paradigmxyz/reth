use std::marker::PhantomData;

use jsonrpsee::http_client::HttpClient;
use reth::{
    api::{EngineTypes, PayloadBuilderAttributes},
    providers::CanonStateNotificationStream,
    rpc::{
        api::EngineApiClient,
        types::engine::{ExecutionPayloadEnvelopeV3, ForkchoiceState},
    },
};
use reth_payload_builder::PayloadId;
use reth_primitives::B256;

/// Helper for engine api operations
pub struct EngineApiHelper<E>
where
    E: EngineTypes<ExecutionPayloadV3 = ExecutionPayloadEnvelopeV3> + 'static,
{
    pub canonical_stream: CanonStateNotificationStream,
    pub engine_api_client: HttpClient,
    pub _marker: PhantomData<E>,
}

impl<E: EngineTypes<ExecutionPayloadV3 = ExecutionPayloadEnvelopeV3> + 'static> EngineApiHelper<E>
where
    ExecutionPayloadEnvelopeV3: From<<E as EngineTypes>::BuiltPayload>,
{
    /// Retrieves a v3 payload from the engine api
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<ExecutionPayloadEnvelopeV3> {
        Ok(EngineApiClient::<E>::get_payload_v3(&self.engine_api_client, payload_id).await?)
    }

    /// Submits a payload to the engine api
    pub async fn submit_payload(
        &self,
        payload: E::BuiltPayload,
        eth_attr: E::PayloadBuilderAttributes,
    ) -> eyre::Result<B256> {
        // setup payload for submission
        let envelope_v3 = ExecutionPayloadEnvelopeV3::from(payload);
        let payload_v3 = envelope_v3.execution_payload;

        // submit payload to engine api
        let submission = EngineApiClient::<E>::new_payload_v3(
            &self.engine_api_client,
            payload_v3,
            vec![],
            eth_attr.parent_beacon_block_root().unwrap(),
        )
        .await?;
        assert!(submission.is_valid(), "{}", submission);
        Ok(submission.latest_valid_hash.unwrap())
    }

    /// Sends forkchoice update to the engine api
    pub async fn update_forkchoice(&self, hash: B256) -> eyre::Result<()> {
        EngineApiClient::<E>::fork_choice_updated_v2(
            &self.engine_api_client,
            ForkchoiceState {
                head_block_hash: hash,
                safe_block_hash: hash,
                finalized_block_hash: hash,
            },
            None,
        )
        .await?;

        Ok(())
    }
}
