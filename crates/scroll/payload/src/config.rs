//! Configuration for the payload builder.

/// Settings for the Scroll builder.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ScrollBuilderConfig {
    /// Desired gas limit.
    pub desired_gas_limit: u64,
}

impl ScrollBuilderConfig {
    /// Returns a new instance of [`ScrollBuilderConfig`].
    pub const fn new(desired_gas_limit: u64) -> Self {
        Self { desired_gas_limit }
    }
}
