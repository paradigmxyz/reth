#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO rm later
#![allow(missing_docs, unreachable_pub, unused)]

//! Implementation of [EIP-1459](https://eips.ethereum.org/EIPS/eip-1459) Node Discovery via DNS.

use enr::Enr;
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap},
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
    error::ParseEntryResult,
    query::{QueryOutcome, QueryPool, ResolveEntryResult, ResolveRootResult},
    sync::{ResolveKind, SyncAction},
    tree::LinkEntry,
};
pub use config::DnsDiscoveryConfig;
use error::ParseDnsEntryError;
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
    /// All subscribers for event updates.
    update_listeners: Vec<mpsc::Sender<DnsDiscoveryUpdate>>,
    /// All the trees that can be synced.
    trees: HashMap<LinkEntry, SyncTree>,
    /// All queries currently in progress
    queries: QueryPool<R, SecretKey>,

    entries: HashMap<String, Enr<SecretKey>>,
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
    pub fn new(resolver: Arc<R>, config: DnsDiscoveryConfig) -> Self {
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

    /// Creates a new channel for [`DnsDiscoveryUpdate`]s.
    pub fn update_stream(&mut self) -> ReceiverStream<DnsDiscoveryUpdate> {
        let (tx, rx) = mpsc::channel(256);
        self.update_listeners.push(tx);
        ReceiverStream::new(rx)
    }

    /// Sends  the event to all listeners.
    ///
    /// Remove channels that got closed.
    fn notify(&mut self, update: DnsDiscoveryUpdate) {
        self.update_listeners.retain_mut(|listener| match listener.try_send(update.clone()) {
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
        if self.entries.contains_key(&hash) {
            // already resolved
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
                Entry::Vacant(mut entry) => {
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

    fn on_resolved_entry(&mut self, resp: ResolveEntryResult<SecretKey>) {
        let ResolveEntryResult { entry, link, hash, kind } = resp;
        match entry {
            Some(Err(err)) => {
                debug!(?err, domain=%link.domain, ?hash, "Failed to parse entry")
            }
            None => {
                debug!(domain=%link.domain, ?hash, "Failed to lookup entry")
            }
            Some(Ok(entry)) => {}
        }
    }

    /// Advances the state of the DNS discovery service by polling,triggering lookups
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DnsDiscoveryEvent> {
        loop {
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

            if !progress {
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

/// Represents [NodeRecord] related discovery updates
#[derive(Debug, Clone)]
pub enum DnsDiscoveryUpdate {}

/// Represents dns discovery related update events.
#[derive(Debug, Clone)]
pub enum DnsDiscoveryEvent {}
