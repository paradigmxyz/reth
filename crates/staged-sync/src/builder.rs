use super::config::HeadersConfig;
use reth_db::database::Database;
use reth_downloaders::headers::linear::{LinearDownloadBuilder, LinearDownloader};
use reth_interfaces::consensus::Consensus;
use reth_network::{FetchClient, NetworkHandle};
use reth_stages::{metrics::HeaderMetrics, stages::headers::HeaderStage, Pipeline};

use std::sync::Arc;

use eyre::Result;

/// RethBuilder::new(..).online(..).with_headers(..).with_bodies(..).with_execution(..).build()
/// RethBuilder::new(..).with_execution(..).build()
#[must_use = "need to call `build` on this struct"]
pub struct RethBuilder<DB> {
    db: Arc<DB>,
}

impl<DB: Database> RethBuilder<DB> {
    pub fn new(db: DB) -> Self {
        Self { db: Arc::new(db) }
    }

    pub fn online<C, N>(self, consensus: C, network: N) -> OnlineRethBuilder<C, N, DB> {
        OnlineRethBuilder::new(self, consensus, network)
    }
}

#[must_use = "need to call `build` on this struct"]
pub struct OnlineRethBuilder<C, N, DB> {
    builder: RethBuilder<DB>,
    consensus: Arc<C>,
    network: N,
    headers: Option<HeadersConfig>,
}

impl<C, N, DB: Database> OnlineRethBuilder<C, N, DB> {
    pub fn new(builder: RethBuilder<DB>, consensus: C, network: N) -> OnlineRethBuilder<C, N, DB> {
        Self { builder, consensus: Arc::new(consensus), network, headers: None }
    }
}

impl<C, DB> OnlineRethBuilder<C, NetworkHandle, DB>
where
    C: Consensus + 'static,
    DB: Database,
{
    pub fn with_headers_downloader(mut self, config: HeadersConfig) -> Self {
        self.headers = Some(config);
        self
    }

    /// Returns the currently configured `HeaderStage` if a `HeadersConfig` has been provided.
    pub async fn headers_stage(
        &self,
    ) -> Result<Option<HeaderStage<LinearDownloader<C, FetchClient>, C, FetchClient, NetworkHandle>>>
    {
        let fetch_client = Arc::new(self.network.fetch_client().await?);
        Ok::<_, eyre::Error>(self.headers.as_ref().map(|config| {
            let downloader = LinearDownloadBuilder::default()
                .request_limit(config.downloader_batch_size)
                .stream_batch_size(config.commit_threshold as usize)
                // NOTE: the head and target will be set from inside the stage before the
                // downloader is called
                .build(
                    self.consensus.clone(),
                    fetch_client.clone(),
                    Default::default(),
                    Default::default(),
                );

            HeaderStage {
                downloader,
                consensus: self.consensus.clone(),
                client: fetch_client.clone(),
                network_handle: self.network.clone(),
                metrics: HeaderMetrics::default(),
            }
        }))
    }

    pub async fn build(&self) -> Result<()> {
        // TODO: Add bodies.
        if self.headers.is_none() {
            return Err(eyre::eyre!("No online stages configured, cnosider removing the `online` call from your builder or add a stage"));
        }

        // Instantiate the networked pipeline
        let mut pipeline =
            Pipeline::<DB, _>::default().with_sync_state_updater(self.network.clone());

        if let Some(stage) = self.headers_stage().await? {
            pipeline = pipeline.push(stage);
        }

        pipeline.run(self.builder.db.clone()).await?;

        Ok(())
    }
}
