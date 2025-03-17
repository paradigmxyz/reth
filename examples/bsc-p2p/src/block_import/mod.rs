#![allow(unused)]
use handle::ImportHandle;
use reth_engine_primitives::EngineTypes;
use reth_network::import::BlockImport;
use reth_network_peers::PeerId;
use reth_payload_primitives::BuiltPayload;
use reth_primitives::NodePrimitives;
use service::{BlockMsg, Outcome};
use std::{
    fmt,
    task::{ready, Context, Poll},
};

mod handle;
mod parlia;
mod service;

pub struct BscBlockImport<EngineT: EngineTypes> {
    handle: ImportHandle<EngineT>,
}

impl<EngineT: EngineTypes> BscBlockImport<EngineT> {
    pub fn new(handle: ImportHandle<EngineT>) -> Self {
        Self { handle }
    }
}

impl<EngineT: EngineTypes>
    BlockImport<<<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block>
    for BscBlockImport<EngineT>
{
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: BlockMsg<EngineT>) {
        let _ = self.handle.send_block(incoming_block, peer_id);
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Outcome<EngineT>> {
        match ready!(self.handle.poll_outcome(cx)) {
            Some(outcome) => Poll::Ready(outcome),
            None => Poll::Pending,
        }
    }
}

impl<EngineT: EngineTypes> fmt::Debug for BscBlockImport<EngineT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BscBlockImport")
            .field("engine_handle", &"BeaconConsensusEngineHandle")
            .field("service_handle", &"BscBlockImportHandle")
            .finish()
    }
}
