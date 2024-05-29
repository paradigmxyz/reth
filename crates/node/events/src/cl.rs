//! Events related to Consensus Layer health.

use futures::Stream;
use reth_provider::CanonChainTracker;
use std::{
    fmt,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Interval};

/// Interval of checking Consensus Layer client health.
const CHECK_INTERVAL: Duration = Duration::from_secs(300);
/// Period of not exchanging transition configurations with Consensus Layer client,
/// after which the warning is issued.
const NO_TRANSITION_CONFIG_EXCHANGED_PERIOD: Duration = Duration::from_secs(120);
/// Period of not receiving fork choice updates from Consensus Layer client,
/// after which the warning is issued.
const NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD: Duration = Duration::from_secs(120);

/// A Stream of [ConsensusLayerHealthEvent].
pub struct ConsensusLayerHealthEvents {
    interval: Interval,
    canon_chain: Box<dyn CanonChainTracker>,
}

impl fmt::Debug for ConsensusLayerHealthEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsensusLayerHealthEvents").field("interval", &self.interval).finish()
    }
}

impl ConsensusLayerHealthEvents {
    /// Creates a new [ConsensusLayerHealthEvents] with the given canonical chain tracker.
    pub fn new(canon_chain: Box<dyn CanonChainTracker>) -> Self {
        // Skip the first tick to prevent the false `ConsensusLayerHealthEvent::NeverSeen` event.
        let interval = tokio::time::interval_at(Instant::now() + CHECK_INTERVAL, CHECK_INTERVAL);
        Self { interval, canon_chain }
    }
}

impl Stream for ConsensusLayerHealthEvents {
    type Item = ConsensusLayerHealthEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            ready!(this.interval.poll_tick(cx));

            if let Some(fork_choice) = this.canon_chain.last_received_update_timestamp() {
                if fork_choice.elapsed() <= NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD {
                    // We had an FCU, and it's recent. CL is healthy.
                    continue
                } else {
                    // We had an FCU, but it's too old.
                    return Poll::Ready(Some(
                        ConsensusLayerHealthEvent::HaveNotReceivedUpdatesForAWhile(
                            fork_choice.elapsed(),
                        ),
                    ))
                }
            }

            if let Some(transition_config) =
                this.canon_chain.last_exchanged_transition_configuration_timestamp()
            {
                return if transition_config.elapsed() <= NO_TRANSITION_CONFIG_EXCHANGED_PERIOD {
                    // We never had an FCU, but had a transition config exchange, and it's recent.
                    Poll::Ready(Some(ConsensusLayerHealthEvent::NeverReceivedUpdates))
                } else {
                    // We never had an FCU, but had a transition config exchange, but it's too old.
                    Poll::Ready(Some(ConsensusLayerHealthEvent::HasNotBeenSeenForAWhile(
                        transition_config.elapsed(),
                    )))
                }
            }

            // We never had both FCU and transition config exchange.
            return Poll::Ready(Some(ConsensusLayerHealthEvent::NeverSeen))
        }
    }
}

/// Event that is triggered when Consensus Layer health is degraded from the
/// Execution Layer point of view.
#[derive(Clone, Copy, Debug)]
pub enum ConsensusLayerHealthEvent {
    /// Consensus Layer client was never seen.
    NeverSeen,
    /// Consensus Layer client has not been seen for a while.
    HasNotBeenSeenForAWhile(Duration),
    /// Updates from the Consensus Layer client were never received.
    NeverReceivedUpdates,
    /// Updates from the Consensus Layer client have not been received for a while.
    HaveNotReceivedUpdatesForAWhile(Duration),
}
