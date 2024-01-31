//! Implementation of [EIP-1459](https://eips.ethereum.org/EIPS/eip-1459) Node Discovery via DNS.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub use crate::resolver::{DnsResolver, MapResolver, Resolver};
use crate::{
    query::{QueryOutcome, QueryPool, ResolveEntryResult, ResolveRootResult},
    sync::{ResolveKind, SyncAction},
    tree::{DnsEntry, LinkEntry},
};
pub use config::DnsDiscoveryConfig;
use enr::Enr;
use error::ParseDnsEntryError;
use futures::StreamExt;
use reth_primitives::{ForkId, NodeRecord, NodeRecordParseError};
use schnellru::{ByLength, LruMap};
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    error::Error,
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use sync::SyncTree;
use thiserror::Error;
use tokio::{
    sync::{
        mpsc,
        mpsc::{error::TrySendError, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};
use tokio_stream::{
    wrappers::{ReceiverStream, UnboundedReceiverStream},
    Stream,
};
use tracing::{debug, trace};

mod config;
mod error;
mod query;
pub mod resolver;
mod sync;
pub mod tree;

/// [DnsDiscoveryService] front-end.
#[derive(Clone, Debug)]
pub struct DnsDiscoveryHandle<N> {
    /// Channel for sending commands to the service.
    to_service: UnboundedSender<DnsDiscoveryCommand<N>>,
}

// === impl DnsDiscovery ===

impl<N> DnsDiscoveryHandle<N> {
    /// Starts syncing the given link to a tree.
    pub fn sync_tree(&mut self, link: &str) -> Result<(), ParseDnsEntryError> {
        self.sync_tree_with_link(link.parse()?);
        Ok(())
    }

    /// Starts syncing the given link to a tree.
    pub fn sync_tree_with_link(&mut self, link: LinkEntry) {
        let _ = self.to_service.send(DnsDiscoveryCommand::SyncTree(link));
    }

    /// Returns the receiver half of new listener channel that streams discovered [`NodeRecord`]s.
    pub async fn node_record_stream(
        &self,
    ) -> Result<ReceiverStream<DnsNodeRecordUpdate<N>>, oneshot::error::RecvError> {
        let (tx, rx) = oneshot::channel();
        let cmd = DnsDiscoveryCommand::NodeRecordUpdates(tx);
        let _ = self.to_service.send(cmd);
        rx.await
    }
}

/// A client that discovers nodes via DNS.
#[must_use = "Service does nothing unless polled"]
#[allow(missing_debug_implementations)]
pub struct DnsDiscoveryService<R: Resolver = DnsResolver, N = NodeRecord> {
    /// Copy of the sender half, so new [`DnsDiscoveryHandle`] can be created on demand.
    command_tx: UnboundedSender<DnsDiscoveryCommand<N>>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<DnsDiscoveryCommand<N>>,
    /// All subscribers for resolved node records.
    node_record_listeners: Vec<mpsc::Sender<DnsNodeRecordUpdate<N>>>,
    /// All the trees that can be synced.
    trees: HashMap<LinkEntry, SyncTree>,
    /// All queries currently in progress
    queries: QueryPool<R, SecretKey>,
    /// Cached dns records
    dns_record_cache: LruMap<String, DnsEntry<SecretKey>>,
    /// all buffered events
    queued_events: VecDeque<DnsDiscoveryEvent>,
    /// The rate at which trees should be updated.
    recheck_interval: Duration,
    /// Links to the DNS networks to bootstrap.
    bootstrap_dns_networks: HashSet<LinkEntry>,
}

// === impl DnsDiscoveryService ===

impl<R: Resolver, N> DnsDiscoveryService<R, N> {
    /// Creates a new instance of the [DnsDiscoveryService] using the given settings.
    ///
    /// ```
    /// use reth_dns_discovery::{DnsDiscoveryService, DnsResolver};
    /// use std::sync::Arc;
    /// # fn t() {
    /// let service = DnsDiscoveryService::new(
    ///     Arc::new(DnsResolver::from_system_conf().unwrap()),
    ///     Default::default(),
    /// );
    /// # }
    /// ```
    pub fn new(resolver: Arc<R>, config: DnsDiscoveryConfig) -> Self {
        let DnsDiscoveryConfig {
            lookup_timeout,
            max_requests_per_sec,
            recheck_interval,
            dns_record_cache_limit,
            bootstrap_dns_networks,
        } = config;
        let queries = QueryPool::new(resolver, max_requests_per_sec, lookup_timeout);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        Self {
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            node_record_listeners: Default::default(),
            trees: Default::default(),
            queries,
            dns_record_cache: LruMap::new(ByLength::new(dns_record_cache_limit.get())),
            queued_events: Default::default(),
            recheck_interval,
            bootstrap_dns_networks: bootstrap_dns_networks.unwrap_or_default(),
        }
    }

    /// Spawns this services onto a new task
    ///
    /// Note: requires a running runtime
    pub fn spawn<I>(mut self) -> JoinHandle<()>
    where
        Self: Stream<Item = I>,
        N: Clone + Send + 'static,
        I: fmt::Debug,
    {
        tokio::task::spawn(async move {
            self.bootstrap();

            while let Some(event) = self.next().await {
                trace!(target: "disc::dns", ?event,  "processed");
            }
        })
    }

    /// Starts discovery with all configured bootstrap links
    pub fn bootstrap(&mut self) {
        for link in self.bootstrap_dns_networks.clone() {
            self.sync_tree_with_link(link);
        }
    }

    /// Same as [DnsDiscoveryService::new] but also returns a new handle that's connected to the
    /// service
    pub fn new_pair(resolver: Arc<R>, config: DnsDiscoveryConfig) -> (Self, DnsDiscoveryHandle<N>) {
        let service = Self::new(resolver, config);
        let handle = service.handle();
        (service, handle)
    }

    /// Returns a new [`DnsDiscoveryHandle`] that can send commands to this type.
    pub fn handle(&self) -> DnsDiscoveryHandle<N> {
        DnsDiscoveryHandle { to_service: self.command_tx.clone() }
    }

    /// Creates a new channel for [`NodeRecord`]s.
    pub fn node_record_stream(&mut self) -> ReceiverStream<DnsNodeRecordUpdate<N>> {
        let (tx, rx) = mpsc::channel(256);
        self.node_record_listeners.push(tx);
        ReceiverStream::new(rx)
    }

    /// Sends  the event to all listeners.
    ///
    /// Remove channels that got closed.
    fn notify(&mut self, record: DnsNodeRecordUpdate<N>)
    where
        N: Clone,
    {
        self.node_record_listeners.retain_mut(|listener| match listener.try_send(record.clone()) {
            Ok(()) => true,
            Err(err) => match err {
                TrySendError::Full(_) => true,
                TrySendError::Closed(_) => false,
            },
        });
    }

    /// Starts syncing the given link to a tree.
    pub fn sync_tree(&mut self, link: &str) -> Result<(), ParseDnsEntryError> {
        self.sync_tree_with_link(link.parse()?);
        Ok(())
    }

    /// Starts syncing the given link to a tree.
    pub fn sync_tree_with_link(&mut self, link: LinkEntry) {
        self.queries.resolve_root(link);
    }

    /// Resolves an entry
    fn resolve_entry(
        &mut self,
        link: LinkEntry<SecretKey>,
        hash: String,
        kind: ResolveKind,
    ) -> Result<(), DnsUpdateParseError>
    where
        Self: Update,
    {
        if let Some(entry) = self.dns_record_cache.get(&hash).cloned() {
            // already resolved
            let cached = ResolveEntryResult { entry: Some(Ok(entry)), link, hash, kind };
            self.on_resolved_entry(cached)?;

            return Ok(())
        }
        self.queries.resolve_entry(link, hash, kind);

        Ok(())
    }

    fn on_resolved_root(&mut self, resp: ResolveRootResult<SecretKey>) {
        match resp {
            Ok((root, link)) => match self.trees.entry(link.clone()) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().update_root(root);
                }
                Entry::Vacant(entry) => {
                    entry.insert(SyncTree::new(root, link));
                }
            },
            Err((err, link)) => {
                debug!(target: "disc::dns",?err, ?link, "Failed to lookup root")
            }
        }
    }

    fn on_resolved_entry(
        &mut self,
        resp: ResolveEntryResult<SecretKey>,
    ) -> Result<(), DnsUpdateParseError>
    where
        Self: Update,
    {
        let ResolveEntryResult { entry, link, hash, kind } = resp;

        match entry {
            Some(Err(err)) => {
                debug!(target: "disc::dns",?err, domain=%link.domain, ?hash, "Failed to lookup entry")
            }
            None => {
                debug!(target: "disc::dns",domain=%link.domain, ?hash, "No dns entry")
            }
            Some(Ok(entry)) => {
                // cache entry
                self.dns_record_cache.insert(hash.clone(), entry.clone());

                match entry {
                    DnsEntry::Root(root) => {
                        debug!(target: "disc::dns",%root, domain=%link.domain, ?hash, "resolved unexpected root entry");
                    }
                    DnsEntry::Link(link_entry) => {
                        if kind.is_link() {
                            if let Some(tree) = self.trees.get_mut(&link) {
                                tree.resolved_links_mut().insert(hash, link_entry.clone());
                            }
                            self.sync_tree_with_link(link_entry)
                        } else {
                            debug!(target: "disc::dns",%link_entry, domain=%link.domain, ?hash, "resolved unexpected Link entry");
                        }
                    }
                    DnsEntry::Branch(branch_entry) => {
                        if let Some(tree) = self.trees.get_mut(&link) {
                            tree.extend_children(kind, branch_entry.children)
                        }
                    }
                    DnsEntry::Node(entry) => {
                        if kind.is_link() {
                            debug!(target: "disc::dns",domain=%link.domain, ?hash, "resolved unexpected enr entry");
                        } else {
                            self.on_resolved_enr(entry.enr)?
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// A Stream events, mainly used for debugging
impl<R: Resolver, N> Stream for DnsDiscoveryService<R, N>
where
    Self: Update,
{
    type Item = DnsDiscoveryEvent;

    /// Advances the state of the DNS discovery service by polling,triggering lookups
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // drain buffered events first
            if let Some(event) = self.queued_events.pop_front() {
                return Poll::Ready(Some(event))
            }

            // process all incoming commands
            while let Poll::Ready(Some(cmd)) = Pin::new(&mut self.command_rx).poll_next(cx) {
                match cmd {
                    DnsDiscoveryCommand::SyncTree(link) => {
                        self.sync_tree_with_link(link);
                    }
                    DnsDiscoveryCommand::NodeRecordUpdates(tx) => {
                        let _ = tx.send(self.node_record_stream());
                    }
                }
            }

            while let Poll::Ready(outcome) = self.queries.poll(cx) {
                // handle query outcome
                match outcome {
                    QueryOutcome::Root(resp) => self.on_resolved_root(resp),
                    QueryOutcome::Entry(resp) => {
                        if let Err(err) = self.on_resolved_entry(resp) {
                            debug!(target: "net::dns",
                                err=%err,
                                "failed to resolve entry"
                            );
                        }
                    }
                }
            }

            let mut progress = false;
            let now = Instant::now();
            let mut pending_resolves = Vec::new();
            let mut pending_updates = Vec::new();
            let recheck_interval = self.recheck_interval;
            for tree in self.trees.values_mut() {
                while let Some(action) = tree.poll(now, recheck_interval) {
                    progress = true;
                    match action {
                        SyncAction::UpdateRoot => {
                            pending_updates.push(tree.link().clone());
                        }
                        SyncAction::Enr(hash) => {
                            pending_resolves.push((tree.link().clone(), hash, ResolveKind::Enr));
                        }
                        SyncAction::Link(hash) => {
                            pending_resolves.push((tree.link().clone(), hash, ResolveKind::Link));
                        }
                    }
                }
            }

            for (domain, hash, kind) in pending_resolves {
                if let Err(err) = self.resolve_entry(domain, hash, kind) {
                    debug!(target: "net::dns",
                        err=%err,
                        "failed to resolve entry"
                    );
                }
            }

            for link in pending_updates {
                self.sync_tree_with_link(link)
            }

            if !progress && self.queued_events.is_empty() {
                return Poll::Pending
            }
        }
    }
}

/// Trait for converting an [`Enr`] into an update and sending the update. Trait is implemented
/// for all supported update types.
pub trait Update {
    /// Tries to convert a resolved [`Enr`] into a [`DnsNodeRecordUpdate`]. Notifies all listeners
    /// of update upon successful conversion.
    fn on_resolved_enr(&mut self, enr: Enr<SecretKey>) -> Result<(), DnsUpdateParseError>;
}

impl<R: Resolver> Update for DnsDiscoveryService<R> {
    fn on_resolved_enr(&mut self, enr: Enr<SecretKey>) -> Result<(), DnsUpdateParseError> {
        let update = enr.clone().try_into()?;
        self.notify(update);

        self.queued_events.push_back(DnsDiscoveryEvent::Enr(enr));

        Ok(())
    }
}

impl<R: Resolver> Update for DnsDiscoveryService<R, Enr<SecretKey>> {
    fn on_resolved_enr(&mut self, enr: Enr<SecretKey>) -> Result<(), DnsUpdateParseError> {
        let update = enr.clone().try_into()?;
        self.notify(update);

        self.queued_events.push_back(DnsDiscoveryEvent::Enr(enr));

        Ok(())
    }
}

/// The converted discovered [Enr] object
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DnsNodeRecordUpdate<N = NodeRecord> {
    /// Discovered node and it's addresses
    pub node_record: N,
    /// The forkid of the node, if present in the ENR
    pub fork_id: Option<ForkId>,
}

impl TryFrom<Enr<SecretKey>> for DnsNodeRecordUpdate {
    type Error = DnsUpdateParseError;

    fn try_from(enr: Enr<SecretKey>) -> Result<Self, Self::Error> {
        // fork id is how we know this is an EL node, this isn't spec´d out but by precedent
        let fork_id = get_fork_id(&enr)?;

        let node_record = NodeRecord::try_from(enr)?;

        Ok(DnsNodeRecordUpdate { node_record, fork_id: Some(fork_id) })
    }
}

impl TryFrom<Enr<SecretKey>> for DnsNodeRecordUpdate<Enr<SecretKey>> {
    type Error = DnsUpdateParseError;

    fn try_from(enr: Enr<SecretKey>) -> Result<Self, Self::Error> {
        // fork id is how we know this is an EL node, this isn't spec´d out but by precedent
        let fork_id = get_fork_id(&enr)?;

        Ok(DnsNodeRecordUpdate { node_record: enr, fork_id: Some(fork_id) })
    }
}

fn get_fork_id(enr: &Enr<SecretKey>) -> Result<ForkId, DnsUpdateParseError> {
    use alloy_rlp::Decodable;

    let Some(mut maybe_fork_id) = enr.get(b"eth") else {
        return Err(DnsUpdateParseError::ForkIdMissing)
    };

    let Ok(fork_id) = ForkId::decode(&mut maybe_fork_id) else {
        return Err(DnsUpdateParseError::ForkIdDecodeError(maybe_fork_id.to_vec()))
    };

    Ok(fork_id)
}

/// Conversion from [`Enr`] to [`DnsNodeRecordUpdate`] failed.
#[derive(Debug, Error)]
pub enum DnsUpdateParseError {
    /// Missing key used to identify an execution layer enr.
    #[error("fork id missing on enr, 'eth' key missing")]
    ForkIdMissing,
    /// Failed to decode fork ID rlp value.
    #[error("failed to decode fork id, 'eth': {0:?}")]
    ForkIdDecodeError(Vec<u8>),
    /// Conversion from [`Enr`] into [`NodeRecord`] failed.
    #[error(transparent)]
    NodeRecordParseError(#[from] NodeRecordParseError),
}

/// Commands sent from [DnsDiscoveryHandle] to [DnsDiscoveryService]
enum DnsDiscoveryCommand<N = NodeRecord> {
    /// Sync a tree
    SyncTree(LinkEntry),
    NodeRecordUpdates(oneshot::Sender<ReceiverStream<DnsNodeRecordUpdate<N>>>),
}

/// Represents dns discovery related update events.
#[derive(Debug, Clone)]
pub enum DnsDiscoveryEvent {
    /// Resolved an Enr entry via DNS.
    Enr(Enr<SecretKey>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::TreeRootEntry;
    use alloy_chains::Chain;
    use alloy_rlp::Encodable;
    use enr::{EnrBuilder, EnrKey};
    use futures::StreamExt;
    use reth_primitives::{Hardfork, MAINNET};
    use secp256k1::rand::thread_rng;
    use std::{future::poll_fn, net::Ipv4Addr};

    #[tokio::test]
    async fn test_start_root_sync() {
        reth_tracing::init_test_tracing();

        let secret_key = SecretKey::new(&mut thread_rng());
        let resolver = MapResolver::default();
        let s = "enrtree-root:v1 e=QFT4PBCRX4XQCV3VUYJ6BTCEPU l=JGUFMSAGI7KZYB3P7IZW4S5Y3A seq=3 sig=3FmXuVwpa8Y7OstZTx9PIb1mt8FrW7VpDOFv4AaGCsZ2EIHmhraWhe4NxYhQDlw5MjeFXYMbJjsPeKlHzmJREQE";
        let mut root: TreeRootEntry = s.parse().unwrap();
        root.sign(&secret_key).unwrap();

        let link =
            LinkEntry { domain: "nodes.example.org".to_string(), pubkey: secret_key.public() };
        resolver.insert(link.domain.clone(), root.to_string());

        let mut service: DnsDiscoveryService<MapResolver> =
            DnsDiscoveryService::new(Arc::new(resolver), Default::default());

        service.sync_tree_with_link(link.clone());

        poll_fn(|cx| {
            let _ = service.poll_next_unpin(cx);
            Poll::Ready(())
        })
        .await;

        let tree = service.trees.get(&link).unwrap();
        assert_eq!(tree.root().clone(), root);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_node() {
        reth_tracing::init_test_tracing();

        let secret_key = SecretKey::new(&mut thread_rng());
        let resolver = MapResolver::default();
        let s = "enrtree-root:v1 e=QFT4PBCRX4XQCV3VUYJ6BTCEPU l=JGUFMSAGI7KZYB3P7IZW4S5Y3A seq=3 sig=3FmXuVwpa8Y7OstZTx9PIb1mt8FrW7VpDOFv4AaGCsZ2EIHmhraWhe4NxYhQDlw5MjeFXYMbJjsPeKlHzmJREQE";
        let mut root: TreeRootEntry = s.parse().unwrap();
        root.sign(&secret_key).unwrap();

        let link =
            LinkEntry { domain: "nodes.example.org".to_string(), pubkey: secret_key.public() };
        resolver.insert(link.domain.clone(), root.to_string());

        let mut builder = EnrBuilder::new("v4");
        let mut buf = Vec::new();
        let fork_id = MAINNET.hardfork_fork_id(Hardfork::Frontier).unwrap();
        fork_id.encode(&mut buf);
        builder.ip4(Ipv4Addr::LOCALHOST).udp4(30303).tcp4(30303).add_value(b"eth", &buf);
        let enr = builder.build(&secret_key).unwrap();

        resolver.insert(format!("{}.{}", root.enr_root.clone(), link.domain), enr.to_base64());

        let mut service: DnsDiscoveryService<MapResolver> =
            DnsDiscoveryService::new(Arc::new(resolver), Default::default());

        let mut node_records = service.node_record_stream();

        let task = tokio::task::spawn(async move {
            let record = node_records.next().await.unwrap();
            assert_eq!(record.fork_id, Some(fork_id));
        });

        service.sync_tree_with_link(link.clone());

        let event = poll_fn(|cx| service.poll_next_unpin(cx)).await;

        match event.unwrap() {
            DnsDiscoveryEvent::Enr(discovered) => {
                assert_eq!(discovered, enr);
            }
        }

        poll_fn(|cx| {
            assert!(service.poll_next_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_recheck_tree() {
        reth_tracing::init_test_tracing();

        let config = DnsDiscoveryConfig {
            recheck_interval: Duration::from_millis(750),
            ..Default::default()
        };

        let secret_key = SecretKey::new(&mut thread_rng());
        let resolver = Arc::new(MapResolver::default());
        let s = "enrtree-root:v1 e=QFT4PBCRX4XQCV3VUYJ6BTCEPU l=JGUFMSAGI7KZYB3P7IZW4S5Y3A seq=3 sig=3FmXuVwpa8Y7OstZTx9PIb1mt8FrW7VpDOFv4AaGCsZ2EIHmhraWhe4NxYhQDlw5MjeFXYMbJjsPeKlHzmJREQE";
        let mut root: TreeRootEntry = s.parse().unwrap();
        root.sign(&secret_key).unwrap();

        let link =
            LinkEntry { domain: "nodes.example.org".to_string(), pubkey: secret_key.public() };
        resolver.insert(link.domain.clone(), root.to_string());

        let mut service: DnsDiscoveryService<MapResolver> =
            DnsDiscoveryService::new(Arc::clone(&resolver), config.clone());

        service.sync_tree_with_link(link.clone());

        poll_fn(|cx| {
            assert!(service.poll_next_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        // await recheck timeout
        tokio::time::sleep(config.recheck_interval).await;

        let enr = EnrBuilder::new("v4").build(&secret_key).unwrap();
        resolver.insert(format!("{}.{}", root.enr_root.clone(), link.domain), enr.to_base64());

        let event = poll_fn(|cx| service.poll_next_unpin(cx)).await;

        match event.unwrap() {
            DnsDiscoveryEvent::Enr(discovered) => {
                assert_eq!(discovered, enr);
            }
        }

        poll_fn(|cx| {
            assert!(service.poll_next_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    #[ignore]
    async fn test_dns_resolver() {
        reth_tracing::init_test_tracing();

        let mut service: DnsDiscoveryService = DnsDiscoveryService::new(
            Arc::new(DnsResolver::from_system_conf().unwrap()),
            Default::default(),
        );

        service.sync_tree(&Chain::mainnet().public_dns_network_protocol().unwrap()).unwrap();

        while let Some(event) = service.next().await {
            match event {
                DnsDiscoveryEvent::Enr(enr) => {
                    println!("discovered enr {}", enr.to_base64());
                }
            }
        }
    }
}
