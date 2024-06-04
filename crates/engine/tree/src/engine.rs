//! An engine API handler for the chain.

use crate::chain::{ChainHandler, FromOrchestrator, HandlerEvent, NodeState};
use reth_engine_primitives::EngineTypes;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Advances the chain based on the engine API.
///
/// This type listens for engine API messages and processes them accordingly.
#[derive(Debug)]
pub struct EngineApiChainHandler<EngineT>
where
    EngineT: EngineTypes,
{
    /// The current state of the node.
    state: NodeState,
    // /// The Engine API message receiver.
    // engine_message_stream: BoxStream<'static, BeaconEngineMessage<EngineT>>,

    // TODO keep track of pipeline distance
    // TODO integrate sync here?
}

impl<EngineT> EngineApiChainHandler<EngineT>
where
    EngineT: EngineTypes,
{
    /// Creates a new instance of the chain handler.
    pub fn new() -> Self {
        todo!()
    }
}

impl<EngineT> ChainHandler for EngineApiChainHandler<EngineT>
where
    EngineT: EngineTypes,
{
    fn on_event(&mut self, event: FromOrchestrator) {
        todo!()
    }

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HandlerEvent> {
        todo!()
    }
}
