#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Implementation of [EIP-1459](https://eips.ethereum.org/EIPS/eip-1459) Node Discovery via DNS.

use enr::{Enr};
use lru::LruCache;
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Instant,
};
use tokio::sync::{
    mpsc,
    mpsc::{error::TrySendError, UnboundedSender},
};
use tokio_stream::{
    wrappers::{ReceiverStream, UnboundedReceiverStream},
    Stream,
};
use tracing::{debug, warn};


mod config;
mod error;
mod query;
pub mod resolver;
mod sync;
pub mod tree;

pub use crate::resolver::{DnsResolver, MapResolver, Resolver};
use crate::{
    query::{QueryOutcome, QueryPool, ResolveEntryResult, ResolveRootResult},
    sync::{ResolveKind, SyncAction},
    tree::{DnsEntry, LinkEntry},
};
pub use config::DnsDiscoveryConfig;
use error::ParseDnsEntryError;
use reth_primitives::{NodeRecord, PeerId};
use sync::SyncTree;

/// [DnsDiscoveryService] front-end.
#[derive(Clone)]
pub struct DnsDiscoveryHandle {
    /// Channel for sending commands to the service.
    to_service: UnboundedSender<DnsDiscoveryCommand>,
}

// === impl DnsDiscovery ===

impl DnsDiscoveryHandle {}

/// A client that discovers nodes via DNS.
#[must_use = "Service does nothing unless polled"]
pub struct DnsDiscoveryService<R: Resolver = DnsResolver> {
    /// Copy of the sender half, so new [`DnsDiscoveryHandle`] can be created on demand.
    command_tx: UnboundedSender<DnsDiscoveryCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<DnsDiscoveryCommand>,
    /// All subscribers for resolved [NodeRecord]s.
    node_record_listeners: Vec<mpsc::Sender<NodeRecord>>,
    /// All the trees that can be synced.
    trees: HashMap<LinkEntry, SyncTree>,
    /// All queries currently in progress
    queries: QueryPool<R, SecretKey>,
    /// Cached dns records
    dns_record_cache: LruCache<String, DnsEntry<SecretKey>>,
    /// all buffered events
    queued_events: VecDeque<DnsDiscoveryEvent>,
}

// === impl DnsDiscoveryService ===

impl<R: Resolver> DnsDiscoveryService<R> {
    /// Creates a new instance of the [DnsDiscoveryService] using the given settings.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use reth_dns_discovery::{DnsDiscoveryService, DnsResolver};
    /// # fn t() {
    ///  let service =
    ///             DnsDiscoveryService::new(Arc::new(DnsResolver::from_system_conf().unwrap()), Default::default());
    /// # }
    /// ```
    pub fn new(_resolver: Arc<R>, _config: DnsDiscoveryConfig) -> Self {
        todo!()
    }

    /// Same as [DnsDiscoveryService::new] but also returns a new handle that's connected to the
    /// service
    pub fn new_pair(resolver: Arc<R>, config: DnsDiscoveryConfig) -> (Self, DnsDiscoveryHandle) {
        let service = Self::new(resolver, config);
        let handle = service.handle();
        (service, handle)
    }

    /// Returns a new [`DnsDiscoveryHandle`] that can send commands to this type.
    pub fn handle(&self) -> DnsDiscoveryHandle {
        DnsDiscoveryHandle { to_service: self.command_tx.clone() }
    }

    /// Creates a new channel for [`NodeRecord`]s.
    pub fn node_record_stream(&mut self) -> ReceiverStream<NodeRecord> {
        let (tx, rx) = mpsc::channel(256);
        self.node_record_listeners.push(tx);
        ReceiverStream::new(rx)
    }

    /// Sends  the event to all listeners.
    ///
    /// Remove channels that got closed.
    fn notify(&mut self, record: NodeRecord) {
        self.node_record_listeners.retain_mut(|listener| match listener.try_send(record) {
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
    fn resolve_entry(&mut self, link: LinkEntry<SecretKey>, hash: String, kind: ResolveKind) {
        if let Some(entry) = self.dns_record_cache.get(&hash).cloned() {
            // already resolved
            let cached = ResolveEntryResult { entry: Some(Ok(entry)), link, hash, kind };
            self.on_resolved_entry(cached);
            return
        }
        self.queries.resolve_entry(link, hash, kind)
    }

    fn on_resolved_root(&mut self, resp: ResolveRootResult<SecretKey>) {
        match resp {
            Ok(Ok((root, link))) => match self.trees.entry(link.clone()) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().update_root(root);
                }
                Entry::Vacant(entry) => {
                    entry.insert(SyncTree::new(root, link));
                }
            },
            Ok(Err(err)) => {
                debug!(?err, "Resolved invalid root")
            }
            Err(link) => {
                debug!(?link, "Failed to lookup root")
            }
        }
    }

    fn on_resolved_enr(&mut self, enr: Enr<SecretKey>) {
        if let Some(record) = convert_enr_node_record(&enr) {
            self.notify(record);
        }
        self.queued_events.push_back(DnsDiscoveryEvent::Enr(enr))
    }

    fn on_resolved_entry(&mut self, resp: ResolveEntryResult<SecretKey>) {
        let ResolveEntryResult { entry, link, hash, kind } = resp;
        match entry {
            Some(Err(err)) => {
                debug!(?err, domain=%link.domain, ?hash, "Failed to lookup entry")
            }
            None => {
                debug!(domain=%link.domain, ?hash, "No dns entry")
            }
            Some(Ok(entry)) => {
                // cache entry
                self.dns_record_cache.push(hash.clone(), entry.clone());

                match entry {
                    DnsEntry::Root(root) => {
                        debug!(%root, domain=%link.domain, ?hash, "resolved unexpected root entry");
                    }
                    DnsEntry::Link(link_entry) => {
                        if kind.is_link() {
                            if let Some(tree) = self.trees.get_mut(&link) {
                                tree.resolved_links_mut().insert(hash, link_entry.clone());
                            }
                            self.sync_tree_with_link(link_entry)
                        } else {
                            debug!(%link_entry, domain=%link.domain, ?hash, "resolved unexpected Link entry");
                        }
                    }
                    DnsEntry::Branch(branch_entry) => {
                        if let Some(tree) = self.trees.get_mut(&link) {
                            tree.extend_children(kind, branch_entry.children)
                        }
                    }
                    DnsEntry::Node(entry) => {
                        if kind.is_link() {
                            debug!(domain=%link.domain, ?hash, "resolved unexpected enr entry");
                        } else {
                            self.on_resolved_enr(entry.enr)
                        }
                    }
                }
            }
        }
    }

    /// Advances the state of the DNS discovery service by polling,triggering lookups
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DnsDiscoveryEvent> {
        loop {
            // drain buffered events first
            if let Some(event) = self.queued_events.pop_front() {
                return Poll::Ready(event)
            }

            while let Poll::Ready(outcome) = self.queries.poll(cx) {
                // handle query outcome
                match outcome {
                    QueryOutcome::Root(resp) => self.on_resolved_root(resp),
                    QueryOutcome::Entry(resp) => self.on_resolved_entry(resp),
                }
            }

            let mut progress = false;
            let now = Instant::now();
            let mut pending_resolves = Vec::new();
            let mut pending_updates = Vec::new();
            for tree in self.trees.values_mut() {
                while let Some(action) = tree.poll(now) {
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
                self.resolve_entry(domain, hash, kind)
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

/// A Stream events, mainly used for debugging
impl<R: Resolver> Stream for DnsDiscoveryService<R> {
    type Item = DnsDiscoveryEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(ready!(self.get_mut().poll(cx))))
    }
}

/// Commands sent from [DnsDiscoveryHandle] to [DnsDiscoveryService]
enum DnsDiscoveryCommand {}

/// Represents dns discovery related update events.
#[derive(Debug, Clone)]
pub enum DnsDiscoveryEvent {
    Enr(Enr<SecretKey>),
}

/// Converts an [Enr] into a [NodeRecord]
fn convert_enr_node_record(enr: &Enr<SecretKey>) -> Option<NodeRecord> {
    let record = NodeRecord {
        address: enr.ip4().map(IpAddr::from).or_else(|| enr.ip6().map(IpAddr::from))?,
        tcp_port: enr.tcp4().or_else(|| enr.tcp6())?,
        udp_port: enr.udp4().or_else(|| enr.udp6())?,
        id: PeerId::from_slice(&enr.public_key().serialize_uncompressed()[1..]),
    }
    .into_ipv4_mapped();
    Some(record)
}
