use crate::tracker::Tracker;
use std::time::{Duration, Instant};

/// Contains the tracker client as well as additional metadata about the
/// tracker.
#[derive(Debug)]
pub(crate) struct TrackerSession {
    client: Tracker,
    /// If a previous announce contained a tracker_id, it should be included in
    /// next announces. Therefore it is cached here.
    id: Option<String>,
    /// The last announce time is kept here so that we don't request too often.
    last_announce_time: Option<Instant>,
    /// The interval at which we should update the tracker of our progress.
    /// This is set after the first announce request.
    interval: Option<Duration>,
    /// The absolute minimum interval at which we can contact tracker.
    /// This is set after the first announce request.
    min_interval: Option<Duration>,
    /// Each time we fail to request from tracker, this counter is incremented.
    /// If it fails too often, we stop requesting from tracker.
    error_count: usize,
}

impl TrackerSession {
    fn new(client: Tracker) -> Self {
        Self {
            client,
            id: None,
            last_announce_time: None,
            interval: None,
            min_interval: None,
            error_count: 0,
        }
    }

    /// Determines whether we should announce to the tracker at the given time,
    /// based on when we last announced.
    ///
    /// Later this function should take into consideration the client's minimum
    /// announce frequency settings.
    fn should_announce(&self, t: Instant, default_announce_interval: Duration) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            let min_next_announce_time =
                last_announce_time + self.interval.unwrap_or(default_announce_interval);
            t > min_next_announce_time
        } else {
            true
        }
    }

    /// Determines whether we're allowed to announce at the given time.
    ///
    /// We may need peers before the next step in the announce interval.
    /// However, we can't do this too often, so we need to check our last
    /// announce time first.
    fn can_announce(&self, t: Instant, default_announce_interval: Duration) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            let min_next_announce_time = last_announce_time +
                self.min_interval.or(self.min_interval).unwrap_or(default_announce_interval);
            t > min_next_announce_time
        } else {
            true
        }
    }
}
