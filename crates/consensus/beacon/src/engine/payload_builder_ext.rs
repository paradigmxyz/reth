use crate::engine::message::OnForkChoiceUpdated;
use reth_payload_builder::{
    PayloadBuilderAttributes, PayloadBuilderAttributesTrait, PayloadBuilderHandle,
    PayloadBuilderTrait,
};
use reth_primitives::Header;
use reth_rpc_types::engine::{
    ForkchoiceState, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
};

pub trait EnginePayloadBuilderExt: PayloadBuilderTrait {
    /// The input payload attributes type that is used to construct the underlying
    /// [PayloadBuilderTrait::PayloadAttributes] associated type.
    type RpcPayloadAttributes;

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    fn process_payload_attributes(
        &self,
        attrs: Self::RpcPayloadAttributes,
        head: Header,
        state: ForkchoiceState,
    ) -> OnForkChoiceUpdated;
}

impl EnginePayloadBuilderExt for PayloadBuilderHandle<PayloadBuilderAttributes> {
    type RpcPayloadAttributes = PayloadAttributes;

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    fn process_payload_attributes(
        &self,
        attrs: PayloadAttributes,
        head: Header,
        state: ForkchoiceState,
    ) -> OnForkChoiceUpdated {
        // 7. Client software MUST ensure that payloadAttributes.timestamp is greater than timestamp
        //    of a block referenced by forkchoiceState.headBlockHash. If this condition isn't held
        //    client software MUST respond with -38003: `Invalid payload attributes` and MUST NOT
        //    begin a payload build process. In such an event, the forkchoiceState update MUST NOT
        //    be rolled back.
        if attrs.timestamp <= head.timestamp {
            return OnForkChoiceUpdated::invalid_payload_attributes();
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value if
        //    payloadAttributes is not null and the forkchoice state has been updated successfully.
        //    The build process is specified in the Payload building section.
        match PayloadBuilderAttributes::try_new(state.head_block_hash, attrs) {
            Ok(attributes) => {
                // send the payload to the builder and return the receiver for the pending payload
                // id, initiating payload job is handled asynchronously
                let pending_payload_id = self.send_new_payload(attributes);

                // Client software MUST respond to this method call in the following way:
                // {
                //      payloadStatus: {
                //          status: VALID,
                //          latestValidHash: forkchoiceState.headBlockHash,
                //          validationError: null
                //      },
                //      payloadId: buildProcessId
                // }
                //
                // if the payload is deemed VALID and the build process has begun.
                OnForkChoiceUpdated::updated_with_pending_payload_id(
                    PayloadStatus::new(PayloadStatusEnum::Valid, Some(state.head_block_hash)),
                    pending_payload_id,
                )
            }
            Err(_) => OnForkChoiceUpdated::invalid_payload_attributes(),
        }
    }
}
