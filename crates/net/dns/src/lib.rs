#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO rm later
#![allow(missing_docs, unreachable_pub, unused)]

//! Implementation of [EIP-1459](https://eips.ethereum.org/EIPS/eip-1459) Node Discovery via DNS.

use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc,
    mpsc::{error::TrySendError, UnboundedSender},
};
use tokio_stream::{
    wrappers::{ReceiverStream, UnboundedReceiverStream},
    Stream,
};

mod config;
mod error;
pub mod resolver;
mod sync;
pub mod tree;

pub use crate::resolver::{DnsResolver, MapResolver, Resolver};
use crate::{
    sync::{QueryOutcome, QueryPool, ResolveRootResult, SyncTree},
    tree::LinkEntry,
};
pub use config::DnsDiscoveryConfig;
use error::ParseDnsEntryError;

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
}

// === impl DnsDiscoveryService ===

impl<R: Resolver> DnsDiscoveryService<R> {
    /// Creates a new instance of the [DnsDiscoveryService] using the given settings.
    ///
    /// ```
    /// use reth_dns_discovery::{DnsDiscoveryService, DnsResolver};
    /// # fn t() {
    ///  let service =
    ///             DnsDiscoveryService::new(DnsResolver::from_system_conf().unwrap(), Default::default());
    /// # }
    /// ```
    pub fn new(resolver: R, config: DnsDiscoveryConfig) -> Self {
        todo!()
    }

    /// Same as [DnsDiscoveryService::new] but also returns a new handle that's connected to the
    /// service
    pub fn new_pair(resolver: R, config: DnsDiscoveryConfig) -> (Self, DnsDiscoveryHandle) {
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
        let _link: LinkEntry = link.parse()?;

        Ok(())
    }

    /// Resolves an entry
    fn resolve_entry(&mut self, _domain: impl Into<String>, _hash: impl Into<String>) {}

    fn on_resolved_root(&mut self, resp: ResolveRootResult<SecretKey>) {}

    /// Advances the state of the DNS discovery service by polling,triggering lookups
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DnsDiscoveryEvent> {
        while let Poll::Ready(outcome) = self.queries.poll(cx) {
            // handle query outcome
            match outcome {
                QueryOutcome::Root(resp) => self.on_resolved_root(resp),
            }
        }

        Poll::Pending
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
