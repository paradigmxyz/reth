//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

/// Provides a local dev service engine that can be used to run a dev chain.
#[derive(Debug, Default)]
pub struct LocalEngineService {}

impl LocalEngineService {
    /// Constructor for `LocalEngineService`.
    pub const fn new() -> Self {
        Self {}
    }

    /// Starts the local engine service.
    pub async fn start(&self) -> Result<(), EngineServiceError> {
        Ok(())
    }
}
