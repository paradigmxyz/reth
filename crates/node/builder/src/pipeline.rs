use reth_network_p2p::FullClient;
use reth_node_api::PipelineComponent;

#[derive(Clone)]
pub struct PipelineAdapter<C> {
    client: C,
}

impl<C> PipelineComponent for PipelineAdapter<C>
where
    C: FullClient + Send + Sync + Clone + 'static,
{
    type Client = C;
}
