use reth_node_api::PayloadTypes;
use reth_optimism_node::{OpBuiltPayload, OpPayloadAttributes, OpPayloadBuilderAttributes};
use reth_optimism_primitives::OpTransactionSigned;
use crate::primitives::CustomNodePrimitives;

pub struct CustomEngineTypes;

impl PayloadTypes for CustomEngineTypes {
    type BuiltPayload = OpBuiltPayload<CustomNodePrimitives>;
    type PayloadAttributes = OpPayloadAttributes;
    type PayloadBuilderAttributes = OpPayloadBuilderAttributes<OpTransactionSigned>;
}