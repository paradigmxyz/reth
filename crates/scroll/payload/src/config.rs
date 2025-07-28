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
    /// Maximum total data availability size for a block.
    pub max_da_block_size: Option<u64>,
}

/// Minimal data bytes size per transaction.
pub const MIN_TRANSACTION_DATA_SIZE: u64 = 115u64;

impl ScrollBuilderConfig {
    /// Returns a new instance of [`ScrollBuilderConfig`].
    pub const fn new(gas_limit: u64, time_limit: Duration, max_da_block_size: Option<u64>) -> Self {
        Self { gas_limit, time_limit, max_da_block_size }
    }

    /// Returns the [`PayloadBuildingBreaker`] for the config.
    pub(super) fn breaker(&self) -> PayloadBuildingBreaker {
        PayloadBuildingBreaker::new(self.time_limit, self.gas_limit, self.max_da_block_size)
    }
}

/// Used in the [`super::ScrollPayloadBuilder`] to exit the transactions execution loop early.
#[derive(Debug, Clone)]
pub struct PayloadBuildingBreaker {
    start: Instant,
    time_limit: Duration,
    gas_limit: u64,
    max_da_block_size: Option<u64>,
}

impl PayloadBuildingBreaker {
    /// Returns a new instance of the [`PayloadBuildingBreaker`].
    fn new(time_limit: Duration, gas_limit: u64, max_da_block_size: Option<u64>) -> Self {
        Self { start: Instant::now(), time_limit, gas_limit, max_da_block_size }
    }

    /// Returns whether the payload building should stop.
    pub(super) fn should_break(
        &self,
        cumulative_gas_used: u64,
        cumulative_da_size_used: u64,
    ) -> bool {
        // Check time limit
        if self.start.elapsed() >= self.time_limit {
            return true;
        }

        // Check gas limit
        if cumulative_gas_used > self.gas_limit.saturating_sub(MIN_TRANSACTION_GAS) {
            return true;
        }

        // Check data availability size limit if configured
        if let Some(max_size) = self.max_da_block_size {
            if cumulative_da_size_used > max_size.saturating_sub(MIN_TRANSACTION_DATA_SIZE) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_break_on_time_limit() {
        let breaker = PayloadBuildingBreaker::new(
            Duration::from_millis(200),
            2 * MIN_TRANSACTION_GAS,
            Some(2 * MIN_TRANSACTION_DATA_SIZE),
        );
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE));
        std::thread::sleep(Duration::from_millis(201));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE));
    }

    #[test]
    fn test_should_break_on_gas_limit() {
        let breaker = PayloadBuildingBreaker::new(
            Duration::from_secs(1),
            2 * MIN_TRANSACTION_GAS,
            Some(2 * MIN_TRANSACTION_DATA_SIZE),
        );
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS + 1, MIN_TRANSACTION_DATA_SIZE));
    }

    #[test]
    fn test_should_break_on_data_size_limit() {
        let breaker = PayloadBuildingBreaker::new(
            Duration::from_secs(1),
            2 * MIN_TRANSACTION_GAS,
            Some(2 * MIN_TRANSACTION_DATA_SIZE),
        );
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE + 1));
    }

    #[test]
    fn test_should_break_with_no_da_limit() {
        let breaker = PayloadBuildingBreaker::new(
            Duration::from_secs(1),
            2 * MIN_TRANSACTION_GAS,
            None, // No DA limit
        );
        // Should not break on large DA size when no limit is set
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS, u64::MAX));
        // But should still break on gas limit
        assert!(breaker.should_break(MIN_TRANSACTION_GAS + 1, u64::MAX));
    }
}
