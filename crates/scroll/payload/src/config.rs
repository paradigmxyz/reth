//! Configuration for the payload builder.

use core::time::Duration;
use reth_chainspec::MIN_TRANSACTION_GAS;
use std::{fmt::Debug, time::Instant};

/// Settings for the Scroll builder.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ScrollBuilderConfig {
    /// Gas limit.
    pub gas_limit: u64,
    /// Time limit for payload building.
    pub time_limit: Duration,
}

impl ScrollBuilderConfig {
    /// Returns a new instance of [`ScrollBuilderConfig`].
    pub const fn new(gas_limit: u64, time_limit: Duration) -> Self {
        Self { gas_limit, time_limit }
    }

    /// Returns the [`PayloadBuildingBreaker`] for the config.
    pub(super) fn breaker(&self) -> PayloadBuildingBreaker {
        PayloadBuildingBreaker::new(self.time_limit, self.gas_limit)
    }
}

/// Used in the [`super::ScrollPayloadBuilder`] to exit the transactions execution loop early.
#[derive(Debug, Clone)]
pub struct PayloadBuildingBreaker {
    start: Instant,
    time_limit: Duration,
    gas_limit: u64,
}

impl PayloadBuildingBreaker {
    /// Returns a new instance of the [`PayloadBuildingBreaker`].
    fn new(time_limit: Duration, gas_limit: u64) -> Self {
        Self { start: Instant::now(), time_limit, gas_limit }
    }

    /// Returns whether the payload building should stop.
    pub(super) fn should_break(&self, cumulative_gas_used: u64) -> bool {
        self.start.elapsed() >= self.time_limit ||
            cumulative_gas_used > self.gas_limit.saturating_sub(MIN_TRANSACTION_GAS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_break_on_time_limit() {
        let breaker =
            PayloadBuildingBreaker::new(Duration::from_millis(200), 2 * MIN_TRANSACTION_GAS);
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS));
        std::thread::sleep(Duration::from_millis(201));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS));
    }

    #[test]
    fn test_should_break_on_gas_limit() {
        let breaker = PayloadBuildingBreaker::new(Duration::from_secs(1), 2 * MIN_TRANSACTION_GAS);
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS + 1));
    }
}
