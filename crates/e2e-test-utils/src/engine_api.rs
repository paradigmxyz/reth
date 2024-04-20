use crate::traits::PayloadEnvelopeExt;
use jsonrpsee::http_client::HttpClient;
use reth::{
    api::{EngineTypes, PayloadBuilderAttributes},
    providers::CanonStateNotificationStream,
    rpc::{api::EngineApiClient, types::engine::ForkchoiceState},
};
use reth_payload_builder::PayloadId;
use reth_primitives::B256;
use std::marker::PhantomData;

/// Helper for engine api operations
pub struct EngineApiTestContext<E> {
    pub canonical_stream: CanonStateNotificationStream,
    pub engine_api_client: HttpClient,
    pub _marker: PhantomData<E>,
}

impl<E: EngineTypes + 'static> EngineApiTestContext<E> {
    /// Retrieves a v3 payload from the engine api
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<E::ExecutionPayloadV3> {
        Ok(EngineApiClient::<E>::get_payload_v3(&self.engine_api_client, payload_id).await?)
    }

    /// Submits a payload to the engine api
    pub async fn submit_payload(
        &self,
        payload: E::BuiltPayload,
        payload_builder_attributes: E::PayloadBuilderAttributes,
        versioned_hashes: Vec<B256>,
    ) -> eyre::Result<B256>
    where
        E::ExecutionPayloadV3: From<E::BuiltPayload> + PayloadEnvelopeExt,
    {
        // setup payload for submission
        let envelope_v3: <E as EngineTypes>::ExecutionPayloadV3 = payload.into();

        // submit payload to engine api
        let submission = EngineApiClient::<E>::new_payload_v3(
            &self.engine_api_client,
            envelope_v3.execution_payload(),
          versioned_hashes,
            payload_builder_attributes.parent_beacon_block_root().unwrap(),
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
