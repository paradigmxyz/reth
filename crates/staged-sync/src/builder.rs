use super::config::HeadersConfig;
use reth_db::database::Database;
use reth_network::NetworkHandle;
use reth_stages::Pipeline;

pub struct PipelineBuilder<C, DB> {
    consensus: Option<C>,
    db: Option<DB>,
    network: Option<NetworkHandle>,
    headers: Option<HeadersConfig>,
}

impl<C, DB: Database> PipelineBuilder<C, DB> {
    #[must_use]
    pub fn with_consensus(mut self, consensus: C) -> PipelineBuilder<C, DB> {
        self.consensus = Some(consensus);
        self
    }

    #[must_use]
    pub fn with_network(mut self, network: NetworkHandle) -> Self {
        self.network = Some(network);
        self
    }

    #[must_use]
    pub fn with_headers_downloader(mut self, config: HeadersConfig) -> Self {
        self.headers = Some(config);
        self
    }

    fn build(&self) -> Pipeline<DB, NetworkHandle> {
        if let Some(ref config) = self.headers {
            // NB: It maybe would be better to have a `NetworkedPipelineBuilder` which lets
            // constructing the online stages, but we can get away with a runtime check here.
            let Some(ref network) = self.network else {
                panic!("Headers require a network to be configured via the `with_network` function.")
            };
        }
        unimplemented!()
    }
}
