#![allow(unused)]
use handle::ImportHandle;
use reth_engine_primitives::EngineTypes;
use reth_network::import::BlockImport;
use reth_network_peers::PeerId;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives::NodePrimitives;
use service::{BlockMsg, ImportEvent, Outcome};
use std::{
    fmt,
    task::{ready, Context, Poll},
};

mod handle;
mod parlia;
mod service;

pub struct BscBlockImport<T: PayloadTypes> {
    handle: ImportHandle<T>,
}

impl<T: PayloadTypes> BscBlockImport<T> {
    pub fn new(handle: ImportHandle<T>) -> Self {
        Self { handle }
    }
}

impl<T: PayloadTypes>
    BlockImport<<<T::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block>
    for BscBlockImport<T>
{
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: BlockMsg<T>) {
        let _ = self.handle.send_block(incoming_block, peer_id);
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ImportEvent<T>> {
        match ready!(self.handle.poll_outcome(cx)) {
            Some(outcome) => Poll::Ready(outcome),
            None => Poll::Pending,
        }
    }
}

impl<T: PayloadTypes> fmt::Debug for BscBlockImport<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BscBlockImport")
            .field("engine_handle", &"BeaconConsensusEngineHandle")
            .field("service_handle", &"BscBlockImportHandle")
            .finish()
    }
}
