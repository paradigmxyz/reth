use std::sync::{Arc, Mutex};

use reth_beacon_consensus::{
    BeaconConsensusEngine, BeaconConsensusEngineError, BeaconConsensusEngineHandle,
    FullBlockchainTreeEngine,
};
use reth_network_p2p::{BodiesClient, HeadersClient};
use reth_node_api::{EngineComponent, FullNodeComponents};
use tokio::sync::oneshot;

#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    C: HeadersClient + BodiesClient,
    BT: FullBlockchainTreeEngine,
{
    pub engine: Arc<Mutex<BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>>>,
    pub handle: BeaconConsensusEngineHandle<N::EngineTypes>,
    pub shutdown_rx: Option<Arc<Mutex<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>>>,
}

impl<N, BT, C> EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine,
    C: HeadersClient + BodiesClient,
{
    pub fn new(
        engine: Arc<Mutex<BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>>>,
        handle: BeaconConsensusEngineHandle<N::EngineTypes>,
    ) -> Self {
        EngineAdapter { engine, handle, shutdown_rx: None }
    }
}

impl<N, BT, C> EngineComponent<N> for EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine + Send + Sync + Unpin + Clone + 'static,
    C: HeadersClient + BodiesClient + Send + Sync + Unpin + Clone + 'static,
{
    type Engine = Arc<Mutex<BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>>>;
    type Handle = BeaconConsensusEngineHandle<N::EngineTypes>;
    type ShutdownRx = Option<Arc<Mutex<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>>>;

    fn engine(&self) -> &Self::Engine {
        &self.engine
    }

    fn handle(&self) -> &Self::Handle {
        &self.handle
    }

    fn shutdown_rx_mut(&mut self) -> &mut Self::ShutdownRx {
        &mut self.shutdown_rx
    }
}
