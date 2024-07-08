use std::sync::{Arc, OnceLock};

use reth_beacon_consensus::{
    BeaconConsensusEngine, BeaconConsensusEngineError, BeaconConsensusEngineHandle,
    FullBlockchainTreeEngine,
};
use reth_network_p2p::FullClient;
use reth_node_api::{EngineComponent, FullNodeComponents};
use tokio::sync::{oneshot, Mutex};

#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    C: FullClient,
    BT: FullBlockchainTreeEngine,
{
    pub engine: Arc<OnceLock<Mutex<BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>>>>,
    pub handle: BeaconConsensusEngineHandle<N::EngineTypes>,
    pub shutdown_rx: Arc<OnceLock<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>>,
}

impl<N, BT, C> EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine,
    C: FullClient,
{
    pub fn new(
        engine: BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>,
        handle: BeaconConsensusEngineHandle<N::EngineTypes>,
    ) -> Self {
        EngineAdapter {
            engine: Arc::new(OnceLock::from(Mutex::new(engine))),
            handle,
            shutdown_rx: Arc::from(OnceLock::new()),
        }
    }
}

impl<N, BT, C> EngineComponent<N> for EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine + Send + Sync + Unpin + Clone + 'static,
    C: FullClient + Send + Sync + Clone + 'static,
{
    type Engine = Arc<OnceLock<Mutex<BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>>>>;
    type Handle = BeaconConsensusEngineHandle<N::EngineTypes>;
    type ShutdownRx = Arc<OnceLock<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>>;

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
