use crate::{avg::SlidingDurationAvg, block::BLOCK_LEN, counter::ThruputCounters};
use std::time::{Duration, Instant};
use tracing::info;

/// Contains the state of both sides of the connection.
#[derive(Clone, Copy, Debug)]
pub struct SessionState {
    /// The current state of the connection.
    pub connection: ConnectionState,
    /// If we're choked, peer doesn't allow us to download pieces from them.
    pub is_choked: bool,
    /// If we're interested, peer has pieces that we don't have.
    pub is_interested: bool,
    /// If peer is choked, we don't allow them to download pieces from us.
    pub is_peer_choked: bool,
    /// If peer is interested in us, they mean to download pieces that we have.
    pub is_peer_interested: bool,
}

impl Default for SessionState {
    /// By default, both sides of the connection start off as choked and not
    /// interested in the other.
    fn default() -> Self {
        Self {
            connection: Default::default(),
            is_choked: true,
            is_interested: false,
            is_peer_choked: true,
            is_peer_interested: false,
        }
    }
}

/// At any given time, a connection with a peer is in one of the below states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConnectionState {
    /// The peer connection has not yet been connected or it had been connected
    /// before but has been stopped.
    Disconnected,
    /// The state during which the TCP connection is established.
    Connecting,
    /// The state after establishing the TCP connection and exchanging the
    /// initial BitTorrent handshake.
    Handshaking,
    /// This state is optional, it is used to verify that the bitfield exchange
    /// occurrs after the handshake and not later. It is set once the handshakes
    /// are exchanged and changed as soon as we receive the bitfield or the the
    /// first message that is not a bitfield. Any subsequent bitfield messages
    /// are rejected and the connection is dropped, as per the standard.
    AvailabilityExchange,
    /// This is the normal state of a peer session, in which any messages, apart
    /// from the 'handshake' and 'bitfield', may be exchanged.
    Connected,
}

/// The default (and initial) state of a peer session is `Disconnected`.
impl Default for ConnectionState {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// Holds and provides facilities to modify the state of a peer session.
#[derive(Default)]
pub(super) struct SessionContext {
    /// The session state.
    pub(crate) state: SessionState,

    /// Measures various transfer statistics.
    pub(crate) counters: ThruputCounters,

    /// A flag to indicate whether since the previous session tick the state has
    /// changed in a way that requires sending a new message to the torrent
    /// task. If this is true, the peer session needs to send a state update
    /// message to torrent.
    ///
    /// The fields whose update causes this flag to be set are the ones defined
    /// before this field, that is:
    /// - [`Self::state`]
    /// - [`Self::counters`]
    pub(crate) changed: bool,

    /// Whether the session is in slow start.
    ///
    /// To keep up with the transport layer's slow start algorithm (which unlike
    /// its name, exponentially increases window size), a `peer_session` starts
    /// out in slow start as well, wherein the target request queue size is
    /// increased by one every time one of our requests got served, doubling the
    /// queue size with each round trip.
    pub(crate) in_slow_start: bool,

    /// Whether we're in endgame mode. See
    /// [`crate::torrent::TorrentContext::in_end_game`].
    ///
    /// This field is a cache for the one in torrent context, as it's accessed
    /// very frequently and we wouldn't want to incur synchronization costs on
    /// the hot path.
    pub(crate) in_endgame: bool,

    /// The target request queue size is the number of block requests we keep
    /// outstanding to fully saturate the link.
    ///
    /// Each peer session needs to maintain an "optimal request queue size"
    /// value (approximately the bandwidth-delay product), which is the number
    /// of block requests it keeps outstanding to fully saturate the link.
    ///
    /// This value is derived by collecting a running average of the downloaded
    /// bytes per second, as well as the average request latency, to arrive at
    /// the bandwidth-delay product B x D. This value is recalculated every time
    /// we receive a block, in order to always keep the link fully saturated.
    ///
    /// ```text
    /// queue = download_rate * link_latency / 16 KiB
    /// ```
    ///
    /// Only set once we start downloading.
    // TODO: consider changing this to just usize starting at 0 and reset to
    // 0 once download finishes so that it's easier to deal with it (not having
    // to match on it all the time)
    pub(crate) target_request_queue_len: Option<usize>,

    /// The last time some requests were sent to the peer.
    pub(crate) last_outgoing_request_time: Option<Instant>,
    /// Updated with the time of receipt of the most recently received requested
    /// block.
    pub(crate) last_incoming_block_time: Option<Instant>,
    /// Updated with the time of receipt of the most recently uploaded block.
    pub(crate) last_outgoing_block_time: Option<Instant>,
    /// This is the average network round-trip-time between the last issued
    /// a request and receiving the next block.
    ///
    /// Note that it doesn't have to be the same block since peers are not
    /// required to serve our requests in order, so this is more of a general
    /// approximation.
    pub(crate) avg_request_rtt: SlidingDurationAvg,
    pub(crate) request_timed_out: bool,
    pub(crate) timed_out_request_count: usize,

    /// The time the BitTorrent connection was established (i.e. after
    /// handshaking)
    pub(crate) connected_time: Option<Instant>,
}

impl SessionContext {
    /// When we check whether to exist slow start mode we want to allow for some
    /// error margin. This is because there may be "micro-fluctuations" in the
    /// download rate but per second but over a longer time the download rate
    /// may still be increasing significantly.
    const SLOW_START_ERROR_MARGIN: u64 = 10000;

    /// The target request queue size is set to this value once we are able to start
    /// downloading.
    const START_REQUEST_QUEUE_LEN: usize = 4;

    /// The smallest timeout value we can give a peer. Very fast peers will have
    /// an average round-trip-times, so a slight deviation would punish them
    /// unnecessarily. Therefore we use a somewhat larger minimum threshold for
    /// timeouts.
    const MIN_TIMEOUT: Duration = Duration::from_secs(2);

    /// Returns the current request timeout value, based on the running average
    /// of past request round trip times.
    pub(crate) fn request_timeout(&self) -> Duration {
        // we allow up to four times the average deviation from the mean
        let t = self.avg_request_rtt.mean() + 4 * self.avg_request_rtt.deviation();
        t.max(Self::MIN_TIMEOUT)
    }

    /// Updates state to reflect that peer was timed out.
    pub(crate) fn register_request_timeout(&mut self) {
        // peer has timed out, only allow a single outstanding request
        // from now until peer hasn't timed out
        self.target_request_queue_len = Some(1);
        self.timed_out_request_count += 1;
        self.request_timed_out = true;
        self.in_slow_start = false;

        self.changed = true;
    }

    /// Prepares for requesting blocks.
    ///
    /// This should be called after being unchoked and becoming interested.
    pub(crate) fn prepare_for_download(&mut self) {
        debug_assert!(self.state.is_interested);
        debug_assert!(!self.state.is_choked);

        self.in_slow_start = true;
        // reset the target request queue size, which will be adjusted as the
        // download progresses
        self.target_request_queue_len = Some(Self::START_REQUEST_QUEUE_LEN);
    }

    /// Convenience method to set any field in state and to set the [`Self::changed`]
    /// flag.
    #[inline(always)]
    pub(crate) fn update_state(&mut self, f: impl FnOnce(&mut SessionState)) {
        f(&mut self.state);
        self.changed = true;
    }

    /// Convenience method to update connection state and to set the
    /// [`Self::changed`] flag.
    #[inline(always)]
    pub(crate) fn set_connection_state(&mut self, c: ConnectionState) {
        self.state.connection = c;
        self.changed = true;
    }

    /// Updates various statistics around a block download.
    ///
    /// This should be called every time a block is received.
    pub(crate) fn update_download_stats(&mut self, block_len: u32) {
        let now = Instant::now();

        // update request time
        if let Some(last_outgoing_request_time) = &mut self.last_outgoing_request_time {
            // Due to what is presumed to be inconsistencies with the
            // `Instant::now()` API, it happens in rare circumstances that using
            // the regular `duration_since` here panics (#48). I suspect this
            // happens when requests are made a very short interval before this
            // function is called, which is likely in very fast downloads.
            // Either way, we guard against this by defaulting to 0.
            let elapsed_since_last_request =
                now.saturating_duration_since(*last_outgoing_request_time);

            // If we timed out before, check if this request arrived within the
            // timeout window, or outside of it. If it arrived within the
            // window, we can mark peer as having recovered from the timeout.
            if self.request_timed_out && elapsed_since_last_request <= self.request_timeout() {
                self.request_timed_out = false;
            }

            let request_rtt = elapsed_since_last_request;
            self.avg_request_rtt.update(request_rtt);
        }

        self.counters.payload.down += block_len as u64;
        self.last_incoming_block_time = Some(now);

        // if we're in slow-start mode, we need to increase the target queue
        // size every time a block is received
        if self.in_slow_start {
            if let Some(target_request_queue_len) = &mut self.target_request_queue_len {
                *target_request_queue_len += 1;
            }
        }

        self.changed = true;
    }

    pub(crate) fn record_waste(&mut self, block_len: u32) {
        self.counters.waste += block_len as u64;
        self.changed = true;
    }

    pub(crate) fn update_upload_stats(&mut self, block_len: u32) {
        self.last_outgoing_block_time = Some(Instant::now());
        self.counters.payload.up += block_len as u64;

        self.changed = true;
    }

    /// Updates various statistics and session state.
    ///
    /// This should be called every second.
    pub(crate) fn tick(&mut self) {
        self.maybe_exit_slow_start();

        // NOTE: This has to be *after* `maybe_exit_slow_start` and *before*
        // `update_target_request_queue_len`, as the first relies on the round
        // not being concluded yet, while the latter relies on the round being
        // concluded (having this round's download accounted for in the download
        // rate).
        // TODO: can we statically ensure this rather than rely on the comment?
        self.counters.reset();

        // if we're still in the timeout, we don't want to increase
        // the target request queue size
        if !self.request_timed_out {
            self.update_target_request_queue_len();
        }

        // reset the dirty flag
        self.changed = false;
    }

    /// Checks if we need to exit slow start.
    ///
    /// We leave slow start if the download rate has not increased
    /// significantly since the last round.
    fn maybe_exit_slow_start(&mut self) {
        // this only makes sense if we're not choked
        if !self.state.is_choked &&
            self.in_slow_start &&
            self.target_request_queue_len.is_some() &&
            self.counters.payload.down.round() > 0 &&
            self.counters.payload.down.round() + Self::SLOW_START_ERROR_MARGIN <
                self.counters.payload.down.avg()
        {
            self.in_slow_start = false;
        }
    }

    /// Adjusts the target request queue size based on the current download
    /// statistics.
    fn update_target_request_queue_len(&mut self) {
        if let Some(target_request_queue_len) = &mut self.target_request_queue_len {
            let prev_queue_len = *target_request_queue_len;

            // this is only applicable if we're not in slow start, as in slow
            // start mode the request queue is increased with each incoming
            // block
            if !self.in_slow_start {
                let download_rate = self.counters.payload.down.avg();
                // guard against integer truncation and round up as
                // overestimating the link capacity is cheaper than
                // underestimating it
                *target_request_queue_len =
                    ((download_rate + (BLOCK_LEN - 1) as u64) / BLOCK_LEN as u64) as usize;
            }

            // make sure the target doesn't go below 1
            // TODO: make this configurable and also enforce an upper bound
            if *target_request_queue_len < 1 {
                *target_request_queue_len = 1;
            }

            if prev_queue_len != *target_request_queue_len {
                info!(
                    "Request queue changed from {} to {}",
                    prev_queue_len, *target_request_queue_len
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_prepare_for_download() {
        let mut s = SessionContext::default();

        s.state.is_interested = true;
        s.state.is_choked = false;

        s.prepare_for_download();

        assert!(s.target_request_queue_len > Some(0));
        assert!(s.in_slow_start);
    }

    #[test]
    fn should_exit_slow_start() {
        let mut s = SessionContext::default();

        s.state.is_interested = true;
        s.state.is_choked = false;
        s.in_slow_start = true;
        s.target_request_queue_len = Some(1);

        // rate increasing
        s.counters.payload.down += 10 * BLOCK_LEN as u64;
        // should not exit slow start
        s.maybe_exit_slow_start();
        assert!(s.in_slow_start);

        // reset counter for next round
        // download rate using weighed average:
        // 0 + (10 * 16384) / 5 = 32768
        s.counters.payload.down.reset();

        // rate still increasing
        s.counters.payload.down += 10 * BLOCK_LEN as u64;
        // should not exit slow start yet
        s.maybe_exit_slow_start();
        assert!(s.in_slow_start);

        // reset counter for next round
        // download rate using weighed average:
        // 32768 + (10 * 16384) / 5 = 65536
        // (FIXME: in practice this seems to be 58982)
        s.counters.payload.down.reset();

        // this round's increase is much less than that of the previous round,
        // should exit slow start
        s.counters.payload.down += 2 * BLOCK_LEN as u64 + 9000;
        s.maybe_exit_slow_start();
        assert!(!s.in_slow_start);
    }

    #[test]
    fn should_not_update_target_request_queue_in_slow_start() {
        let mut s = SessionContext::default();

        s.state.is_interested = true;
        s.state.is_choked = false;
        s.in_slow_start = true;
        s.target_request_queue_len = Some(1);

        // rate increasing
        s.counters.payload.down += 2 * BLOCK_LEN as u64;

        // reset counter for next round
        s.counters.payload.down.reset();

        // this should be a noop
        s.update_target_request_queue_len();
        assert_eq!(s.target_request_queue_len, Some(1));
    }

    #[test]
    fn should_update_target_request_queue() {
        let mut s = SessionContext::default();

        s.state.is_interested = true;
        s.state.is_choked = false;
        s.in_slow_start = false;
        s.target_request_queue_len = Some(1);

        // rate increasing (make it more than a multiple of the block
        // length to be able to test against integer truncation)
        s.counters.payload.down += 10 * BLOCK_LEN as u64 + 5000;
        // reset counter so that it may be used in the download rate below
        s.counters.payload.down.reset();

        // should update queue size according to:
        // download rate using weighed average:
        // 0 + (10 * 16384 + 5000) / 5 = 33768
        // queue size based on bandwidth-delay product:
        // (33768 + (16384 - 1)) / 16384 = 3.06 ~ 3
        s.update_target_request_queue_len();
        assert_eq!(s.target_request_queue_len, Some(3));
    }

    #[test]
    fn should_update_download_stats_in_slow_start() {
        let mut s = SessionContext::default();

        s.state.is_interested = true;
        s.state.is_choked = false;
        s.in_slow_start = true;
        s.target_request_queue_len = Some(1);

        s.update_download_stats(BLOCK_LEN);

        // request queue length should be increased by one in slow start
        assert_eq!(s.target_request_queue_len, Some(2));
        // incoming request time should be set
        assert!(s.last_incoming_block_time.is_some());
        // download stat should be increased
        assert_eq!(s.counters.payload.down.round(), BLOCK_LEN as u64);
    }
}
