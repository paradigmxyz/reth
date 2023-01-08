#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Implementation of [EIP-1459](https://eips.ethereum.org/EIPS/eip-1459) Node Discovery via DNS.

mod config;
mod record;

pub use config::DnsDiscoveryConfig;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, mpsc::UnboundedSender};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};
use tracing::trace;

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
pub struct DnsDiscoveryService {
    /// Copy of the sender half, so new [`DnsDiscoveryHandle`] can be created on demand.
    command_tx: UnboundedSender<DnsDiscoveryCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<DnsDiscoveryCommand>,
    /// All subscribers for event updates.
    event_listener: Vec<mpsc::Sender<DnsDiscoveryEvent>>,
}

// === impl DnsDiscoveryService ===

impl DnsDiscoveryService {
    /// Creates a new instance of the [DnsDiscoveryService] using the given settings.
    pub fn new(config: DnsDiscoveryConfig) -> Self {
        todo!()
    }

    /// Same as [DnsDiscoveryService::new] but also returns a new handle that's connected to the
    /// service
    pub fn new_pair(config: DnsDiscoveryConfig) -> (Self, DnsDiscoveryHandle) {
        let service = Self::new(config);
        let handle = service.handle();
        (service, handle)
    }

    /// Returns a new [`DnsDiscoveryHandle`] that can send commands to this type.
    pub fn handle(&self) -> DnsDiscoveryHandle {
        DnsDiscoveryHandle { to_service: self.command_tx.clone() }
    }

    /// Creates a new channel for [`DiscoveryUpdate`]s.
    pub fn event_listener(&mut self) -> ReceiverStream<DnsDiscoveryEvent> {
        let (tx, rx) = mpsc::channel(256);
        self.event_listener.push(tx);
        ReceiverStream::new(rx)
    }

    /// Sends  the event to all listeners.
    ///
    /// Remove channels that got closed.
    fn notify(&mut self, event: DnsDiscoveryEvent) {
        self.event_listener.retain(|listener| {
            let open = listener.send(event.clone()).is_ok();
            if !open {
                trace!(target : "dns", "event listener channel closed",);
            }
            open
        });
    }

    /// Advances the state of the DNS discovery service by polling,triggering lookups
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}

enum DnsDiscoveryCommand {}

/// Represents dns discovery related update events.
#[derive(Debug, Clone)]
pub enum DnsDiscoveryEvent {}
