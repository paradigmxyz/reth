//! Events related to Consensus Layer health.

use futures::Stream;
use reth_provider::CanonChainTracker;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Interval;

/// Interval of checking Consensus Layer client health.
const CHECK_INTERVAL: Duration = Duration::from_secs(300);
/// Period of not exchanging transition configurations with Consensus Layer client,
/// after which the warning is issued.
const NO_TRANSITION_CONFIG_EXCHANGED_PERIOD: Duration = Duration::from_secs(120);
/// Period of not receiving fork choice updates from Consensus Layer client,
/// after which the warning is issued.
const NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD: Duration = Duration::from_secs(120);

/// A Stream of [ConsensusLayerHealthEvent].
pub struct ConsensusLayerHealthEvents<CC> {
    interval: Interval,
    canon_chain: CC,
}

impl<CC> ConsensusLayerHealthEvents<CC> {
    /// Creates a new [ConsensusLayerHealthEvents] with the given canonical chain tracker.
    pub fn new(canon_chain: CC) -> Self {
        Self { interval: tokio::time::interval(CHECK_INTERVAL), canon_chain }
    }
}

impl<CC> Stream for ConsensusLayerHealthEvents<CC>
where
    CC: CanonChainTracker + Unpin,
{
    type Item = ConsensusLayerHealthEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.interval.poll_tick(cx).is_ready() {
            // this ensures the interval will be polled periodically, see [Interval::poll_tick]
            let _ = this.interval.poll_tick(cx);

            return match (
                this.canon_chain.last_exchanged_transition_configuration_timestamp(),
                this.canon_chain.last_received_update_timestamp(),
            ) {
                (None, _) => Poll::Ready(Some(ConsensusLayerHealthEvent::NeverSeen)),
                (Some(transition_config), _)
                    if transition_config.elapsed() > NO_TRANSITION_CONFIG_EXCHANGED_PERIOD =>
                {
                    Poll::Ready(Some(ConsensusLayerHealthEvent::HasNotBeenSeenForAWhile(
                        transition_config.elapsed(),
                    )))
                }
                (Some(_), None) => {
                    Poll::Ready(Some(ConsensusLayerHealthEvent::NeverReceivedUpdates))
                }
                (Some(_), Some(update))
                    if update.elapsed() > NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD =>
                {
                    Poll::Ready(Some(ConsensusLayerHealthEvent::HaveNotReceivedUpdatesForAWhile(
                        update.elapsed(),
                    )))
                }
                _ => Poll::Pending,
            }
        }

        Poll::Pending
    }
}

/// Event that is triggered when Consensus Layer health is degraded from the
/// Execution Layer point of view.
#[derive(Debug)]
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
