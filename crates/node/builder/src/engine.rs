use reth_beacon_consensus::{BeaconConsensusEngine, FullBlockchainTreeEngine};
use reth_network_p2p::{BodiesClient, HeadersClient};
use reth_node_api::{EngineComponent, FullNodeComponents};
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
pub struct EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    C: HeadersClient + BodiesClient,
    BT: FullBlockchainTreeEngine,
{
    pub engine: BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>,
    pub handle: BeaconConsensusEngineHandle<N::EngineTypes>,
    pub shutdown_signal: Option<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>,
}

impl<N, BT, C> EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine,
    C: HeadersClient + BodiesClient,
{
    pub fn new(
        engine: BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>,
        handle: BeaconConsensusEngineHandle<N::EngineTypes>,
    ) -> Self {
        EngineAdapter { engine, handle, shutdown_signal: None }
    }
}

impl<N, BT, C> EngineComponent<N, BT, C> for EngineAdapter<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine,
    C: HeadersClient + BodiesClient,
{
    type Engine = BeaconConsensusEngine<N::DB, BT, C, N::EngineTypes>;
    type Handle = BeaconConsensusEngineHandle<N::EngineTypes>;
    type ShutdownSignalRx = Option<oneshot::Receiver<Result<(), BeaconConsensusEngineError>>>;

    fn engine(&self) -> &Self::Engine {
        &self.engine
    }

    fn handle(&self) -> &Self::Handle {
        &self.handle
    }

    fn shutdown_signal_rx(&self) -> &Self::ShutdownSignalRx {
        self.shutdown_signal.as_ref()
    }
}
