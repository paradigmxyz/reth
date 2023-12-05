pub use discv5::{
    enr::CombinedKey, service::Service, Config as Discv5Config,
    ConfigBuilder as Discv5ConfigBuilder, Discv5, Enr, Event,
};
use futures_util::StreamExt;
use std::{
    default::Default,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream};

fn allow_all_enrs(_enr: &Enr) -> bool {
    true
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
