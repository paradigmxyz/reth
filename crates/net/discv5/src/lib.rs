pub use discv5::{
    enr::CombinedKey, service::Service, Config as Discv5Config,
    ConfigBuilder as Discv5ConfigBuilder, Discv5, Enr, Event,
};
use futures_util::StreamExt;
use std::{
    default::Default,
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use tokio_stream::{wrappers::ReceiverStream, Stream};

// Wrapper struct for Discv5
pub struct Discv5Wrapper {
    inner: Arc<Mutex<Discv5>>,
}

impl Discv5Wrapper {
    // Constructor to create a new Discv5Wrapper
    pub fn new(discv5: Discv5) -> Self {
        Discv5Wrapper { inner: Arc::new(Mutex::new(discv5)) }
    }

    pub fn convert_to_discv5(self) -> Arc<Mutex<Discv5>> {
        self.inner
    }

    // Start the Discv5 service
    pub async fn start_service(&self) -> Result<(), Discv5Error> {
        let mut discv5 = self.inner.lock().await;
        discv5.start().await.map_err(|e| Discv5Error::Discv5Construct(e.to_string()))
    }

    // Create the event stream
    pub async fn create_event_stream(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<Event>, Discv5Error> {
        let discv5 = self.inner.lock().await;
        discv5.event_stream().await.map_err(|e| Discv5Error::Discv5EventStreamStart(e.to_string()))
    }

    // Method to get a mutable reference to Discv5 from Discv5Wrapper
    pub async fn get_mut(&self) -> MutexGuard<'_, Discv5> {
        self.inner.lock().await
    }
}

impl fmt::Debug for Discv5Wrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Discv5Wrapper(<Discv5>)")
    }
}

/// The default table filter that results in all nodes being accepted into the local routing table.
const fn allow_all_enrs(_enr: &Enr) -> bool {
    true
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Discv5Error {
    /// Failed to decode a key from a table.
    #[error("failed to parse secret key to Signing Key {0}")]
    SecretKeyDecode(String),
    /// Failed to construct a new EnrBuilder
    #[error("failed to constuct a new EnrBuilder {0}")]
    EnrBuilderConstruct(String),
    /// Failed to construct Discv5 instance
    #[error("failed to construct a new Discv5 instance {0}")]
    Discv5Construct(String),
    /// Failed to create a event stream
    #[error("failed to create event stream {0}")]
    Discv5EventStream(String),
    /// Failed to start Discv5 event stream
    #[error("failed to start event stream {0}")]
    Discv5EventStreamStart(String),
}

pub fn default_discv5_config() -> Discv5Config {
    Discv5Config {
        enable_packet_filter: Default::default(),
        request_timeout: Default::default(),
        vote_duration: Default::default(),
        query_peer_timeout: Default::default(),
        query_timeout: Default::default(),
        request_retries: Default::default(),
        session_timeout: Default::default(),
        session_cache_capacity: Default::default(),
        enr_update: Default::default(),
        max_nodes_response: Default::default(),
        enr_peer_update_min: Default::default(),
        query_parallelism: Default::default(),
        ip_limit: Default::default(),
        incoming_bucket_limit: Default::default(),
        table_filter: allow_all_enrs,
        ping_interval: Default::default(),
        report_discovered_peers: Default::default(),
        filter_rate_limiter: Default::default(),
        filter_max_nodes_per_ip: Default::default(),
        filter_max_bans_per_ip: Default::default(),
        permit_ban_list: Default::default(),
        ban_duration: Default::default(),
        executor: Default::default(),
        listen_config: Default::default(),
    }
}
pub struct Discv5Service {
    inner: ReceiverStream<Event>,
}

impl Discv5Service {
    // A constructor to create a new Discv5Service
    pub fn new(event_receiver: mpsc::Receiver<Event>) -> Self {
        Discv5Service { inner: ReceiverStream::new(event_receiver) }
    }
}

impl Stream for Discv5Service {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let receiver = self.get_mut().inner.poll_next_unpin(cx);
        receiver
    }
}
