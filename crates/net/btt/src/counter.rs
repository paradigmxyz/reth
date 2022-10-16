// there are some APIs that are not being used at the moment but are going to be
// used when new features are added
#![allow(dead_code)]
use std::ops::AddAssign;

/// Counts statistics about the communication channels used in torrents.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ThruputCounters {
    /// Counts protocol chatter, which are the exchanged non-payload related
    /// messages (such as 'unchoke', 'have', 'request', etc).
    pub(crate) protocol: ChannelCounter,
    /// Counts the exchanged block bytes. This only include the block's data,
    /// minus the header, which counts towards the protocol chatter.
    pub(crate) payload: ChannelCounter,
    /// Counts the (downloaded) payload bytes that were wasted (i.e. duplicate
    /// blocks that had to be discarded).
    pub(crate) waste: Counter,
}

impl ThruputCounters {
    /// Resets the per-round accummulators of the counters.
    ///
    /// This should be called once a second to provide accurate per second
    /// thruput rates.
    pub(crate) fn reset(&mut self) {
        self.protocol.reset();
        self.payload.reset();
        self.waste.reset();
    }
}

impl AddAssign<&ThruputCounters> for ThruputCounters {
    fn add_assign(&mut self, rhs: &ThruputCounters) {
        self.protocol += &rhs.protocol;
        self.payload += &rhs.payload;
        self.waste += rhs.waste.round();
    }
}

/// Counts statistics about a communication channel (such as protocol chatter or
/// payload transfer), both the ingress and engress sides.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ChannelCounter {
    pub(crate) down: Counter,
    pub(crate) up: Counter,
}

impl ChannelCounter {
    /// Resets the per-round accummulators of the counters.
    ///
    /// This should be called once a second to provide accurate per second
    /// thruput rates.
    pub(crate) fn reset(&mut self) {
        self.down.reset();
        self.up.reset();
    }
}

impl AddAssign<&ChannelCounter> for ChannelCounter {
    fn add_assign(&mut self, rhs: &ChannelCounter) {
        self.down += rhs.down.round();
        self.up += rhs.up.round();
    }
}

/// Used for counting the running average of throughput rates.
///
/// This counts the total bytes transferred, as well as the current round's
/// tally. Then, at the end of each round, the caller is responsible for calling
/// [`Counter::reset`] which updates the running average and clears the
/// per round counter.
///
/// The tallied throughput rate is the 5 second weighed running average. It is
/// produced as follows:
///
/// avg = (avg * 4/5) + (this_round / 5)
///
/// This way a temporary deviation in one round does not punish the overall
/// download rate disproportionately.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counter {
    total: u64,
    round: u64,
    avg: f64,
    peak: f64,
}

impl Counter {
    // TODO: turn this into a const generic parameter once that's supported
    const WEIGHT: u64 = 5;

    /// Records some bytes that were transferred.
    pub(crate) fn add(&mut self, bytes: u64) {
        self.total += bytes;
        self.round += bytes;
    }

    /// Finishes counting this round and updates the 5 second moving average.
    ///
    /// # Important
    ///
    /// This assumes that this function is called once a second.
    pub(crate) fn reset(&mut self) {
        // https://github.com/arvidn/libtorrent/blob/master/src/stat.cpp
        self.avg = (self.avg * (Self::WEIGHT - 1) as f64 / Self::WEIGHT as f64) +
            (self.round as f64 / Self::WEIGHT as f64);
        self.round = 0;

        if self.avg > self.peak {
            self.peak = self.avg;
        }
    }

    /// Returns the 5 second moving average, rounded to the nearest integer.
    pub(crate) fn avg(&self) -> u64 {
        self.avg.round() as u64
    }

    /// Returns the average recorded so far, rounded to the nearest integer.
    pub(crate) fn peak(&self) -> u64 {
        self.peak.round() as u64
    }

    /// Returns the total number recorded.
    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    /// Returns the number recorded in the current round.
    pub(crate) fn round(&self) -> u64 {
        self.round
    }
}

impl AddAssign<u64> for Counter {
    fn add_assign(&mut self, rhs: u64) {
        self.add(rhs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter() {
        let mut c = Counter::default();

        assert_eq!(c.avg(), 0);
        assert_eq!(c.peak(), 0);
        assert_eq!(c.round(), 0);
        assert_eq!(c.total(), 0);

        c += 5;
        assert_eq!(c.round(), 5);
        assert_eq!(c.total(), 5);

        c.reset();
        // 4 * 0 / 5 + 5 / 5 = 1
        assert_eq!(c.avg(), 1);
        assert_eq!(c.peak(), 1);
        assert_eq!(c.round(), 0);
        assert_eq!(c.total(), 5);

        c += 10;
        assert_eq!(c.round(), 10);
        assert_eq!(c.total(), 15);

        c.reset();
        // 4 * 1 / 5 + 10 / 5 = 0.8 + 2 = 2.8 ~ 3
        assert_eq!(c.avg(), 3);
        assert_eq!(c.peak(), 3);
        assert_eq!(c.round(), 0);
        assert_eq!(c.total(), 15);

        c += 30;
        assert_eq!(c.round(), 30);
        assert_eq!(c.total(), 45);

        c.reset();
        // 4 * 2.8 / 5 + 30 / 5 = 2.24 + 6 = 8.24 ~ 8
        assert_eq!(c.avg(), 8);
        assert_eq!(c.peak(), 8);
        assert_eq!(c.round(), 0);
        assert_eq!(c.total(), 45);

        c += 1;
        assert_eq!(c.round(), 1);
        assert_eq!(c.total(), 46);

        c.reset();
        // 4 * 8.24 / 5 + 1 / 5 = 6.592 + 0.2 = 6.792 ~ 7
        assert_eq!(c.avg(), 7);
        assert_eq!(c.peak(), 8);
        assert_eq!(c.round(), 0);
        assert_eq!(c.total(), 46);
    }
}
