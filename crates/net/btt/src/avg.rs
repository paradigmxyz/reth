// there are some APIs that are not being used at the moment but are going to be
// used when new features are added
#![allow(dead_code)]
use std::{convert::TryInto, time::Duration};

/// This is an exponential moving average accumulator.
///
/// An algorithm is used that addresss the initial bias that occurs when all
/// values are initialized with zero or with the first sample (which would bias
/// the average toward the first value). This is achieved by initially giving
/// a low gain for the average and slowly increasing it until the inverted gain
/// is reached.
///
/// For example, the first sample should have a gain of 1 as the average has no
/// meaning.  When adding the second sample, the average has some meaning, but
/// since it only has one sample in it, the gain should be low. In the next
/// round however, the gain may be larger. This increase is repeated until
/// inverted gain is reached.  This way, even early samples have a reasonable
/// impact on the average, which is important in a torrent app.
///
/// Ported from libtorrent: https://blog.libtorrent.org/2014/09/running-averages/
#[derive(Debug)]
pub(crate) struct SlidingAvg {
    /// The current running average, effectively the mean.
    ///
    /// This is a fixed-point value. The sample is multiplied by 64 before
    /// adding it. When the mean is returned, 32 is added and the sum is divided
    /// back by 64, to eliminate integer truncation that would result in a bias.
    /// Fixed-point calculation is used as the alternative is using floats which
    /// is slower as well as more cumbersome to perform conversions on (the main
    /// use case is with integers).
    mean: i64,
    /// The average deviation.
    ///
    /// This is a fixed-point value. The sample is multiplied by 64 before
    /// adding it. When the mean is returned, 32 is added and the sum is divided
    /// back by 64, to eliminate integer truncation that would result in a bias.
    /// Fixed-point calculation is used as the alternative is using floats which
    /// is slower as well as more cumbersome to perform conversions on (the main
    /// use case is with integers).
    deviation: i64,
    /// The number of samples received, but no more than `inverted_gain`.
    sample_count: usize,
    /// This is the threshold used for determining how many initial samples to
    /// give a higher gain than the current average.
    // TODO: turn this into a const generic parameter once that's supported
    inverted_gain: usize,
}

impl SlidingAvg {
    pub(crate) fn new(inverted_gain: usize) -> Self {
        Self { mean: 0, deviation: 0, sample_count: 0, inverted_gain }
    }

    pub(crate) fn update(&mut self, mut sample: i64) {
        // see comment in `Self::mean`
        sample *= 64;

        let deviation = if self.sample_count > 0 { (self.mean - sample).abs() } else { 0 };

        if self.sample_count < self.inverted_gain {
            self.sample_count += 1;
        }

        self.mean += (sample - self.mean) / self.sample_count as i64;

        if self.sample_count > 1 {
            self.deviation += (deviation - self.deviation) / (self.sample_count - 1) as i64;
        }
    }

    pub(crate) fn mean(&self) -> i64 {
        if self.sample_count == 0 {
            0
        } else {
            (self.mean + 32) / 64
        }
    }

    pub(crate) fn deviation(&self) -> i64 {
        if self.sample_count == 0 {
            0
        } else {
            (self.deviation + 32) / 64
        }
    }
}

impl Default for SlidingAvg {
    /// Creates a sliding average with an inverted gain of 20.
    fn default() -> Self {
        Self::new(20)
    }
}

/// Wraps a [`SlidingAvg`] instance and converts the statistics to
/// [`std::time::Duration`] units (keeping everything in the underlying layer as
/// milliseconds).
#[derive(Debug)]
pub(crate) struct SlidingDurationAvg(SlidingAvg);

impl SlidingDurationAvg {
    pub(crate) fn new(inverted_gain: usize) -> Self {
        Self(SlidingAvg::new(inverted_gain))
    }

    pub(crate) fn update(&mut self, sample: Duration) {
        // TODO: is this safe? Duration::from_millis takes u64 but as_millis
        // returns u128 so it's not clear
        let ms = sample.as_millis().try_into().expect("Millisecond overflow");
        self.0.update(ms);
    }

    pub(crate) fn mean(&self) -> Duration {
        let ms = self.0.mean() as u64;
        Duration::from_millis(ms)
    }

    pub(crate) fn deviation(&self) -> Duration {
        let ms = self.0.deviation() as u64;
        Duration::from_millis(ms)
    }
}

impl Default for SlidingDurationAvg {
    /// Creates a sliding average with an inverted gain of 20.
    fn default() -> Self {
        Self(SlidingAvg::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sliding_average() {
        let inverted_gain = 4;
        let mut a = SlidingAvg::new(inverted_gain);

        // the first sample should have a weight of 100%
        let sample = 10;
        a.update(sample);
        assert_eq!(a.sample_count, 1);
        assert_eq!(a.mean(), sample);

        // the second sample should have less weight
        let sample = 15;
        a.update(sample);
        assert_eq!(a.sample_count, 2);
        assert_eq!(a.mean(), 13);

        // the third sample even less
        let sample = 20;
        a.update(sample);
        assert_eq!(a.sample_count, 3);
        assert_eq!(a.mean(), 15);

        // The fourth sample reaches the inverted gain. To test that it has an
        // effect on the sample, always choose a sample when from which the
        // current mean is subtracted and divided by the sample count (which now
        // stops increasing) the result is an integer. For simplicity's sake
        // such a sample is chosen as results in the increase of the mean by 1.

        let sample = 19;
        a.update(sample);
        assert_eq!(a.sample_count, 4);
        assert_eq!(a.mean(), 16);

        let sample = 20;
        a.update(sample);
        assert_eq!(a.sample_count, 4);
        assert_eq!(a.mean(), 17);

        let sample = 21;
        a.update(sample);
        assert_eq!(a.sample_count, 4);
        assert_eq!(a.mean(), 18);

        // also make sure that a large sample only increases the mean by a value
        // proportional to its weight: that is, by (mean - sample) / 4
        let sample = 118;
        // increase should be: (118 - 18) / 4 = 25
        a.update(sample);
        assert_eq!(a.mean(), 43);
    }

    #[test]
    fn test_sliding_duration_average() {
        // since the implementation of the moving average is the same as for
        // `SlidSlidingAvg`, we only need to test that the i64 <-> duration
        // conversions are correct
        let mut a = SlidingDurationAvg::default();

        // initially the mean is the same as the first sample
        let sample = Duration::from_secs(10);
        a.update(sample);
        assert_eq!(a.0.sample_count, 1);
        assert_eq!(a.mean(), sample);
    }
}
