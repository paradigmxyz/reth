use futures::Stream;
use reth_provider::CanonChainTracker;
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::time::Interval;

/// Period of not exchanging transition configurations with consensus client,
/// after which the warning is issued.
const NO_TRANSITION_CONFIG_EXCHANGED_PERIOD: Duration = Duration::from_secs(120);
/// Period of not receiving fork choice updates from consensus client,
/// after which the warning is issued.
const NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD: Duration = Duration::from_secs(120);

pub struct ConsensusLayerHealthEvents<CC> {
    interval: Interval,
    canon_chain: CC,
}

impl<CC> ConsensusLayerHealthEvents<CC> {
    pub fn new(interval: Duration, canon_chain: CC) -> Self {
        Self { interval: tokio::time::interval(interval), canon_chain }
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
                    Poll::Ready(Some(ConsensusLayerHealthEvent::HaveNotSeenInAWhile))
                }
                (Some(_), None) => {
                    Poll::Ready(Some(ConsensusLayerHealthEvent::NeverReceivedUpdates))
                }
                (Some(_), Some(update))
                    if update.elapsed() > NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD =>
                {
                    Poll::Ready(Some(ConsensusLayerHealthEvent::HaveNotReceivedUpdatesInAWhile))
                }
                _ => Poll::Pending,
            }
        }

        Poll::Pending
    }
}

#[derive(Debug)]
pub enum ConsensusLayerHealthEvent {
    NeverSeen,
    HaveNotSeenInAWhile,
    NeverReceivedUpdates,
    HaveNotReceivedUpdatesInAWhile,
}
