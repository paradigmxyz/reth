use alloy_primitives::Bytes;

/// Settings for the Ethereum builder.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct EthereumBuilderConfig {
    /// Block extra data.
    pub extra_data: Bytes,
}

impl EthereumBuilderConfig {
    /// Create new payload builder config.
    pub const fn new(extra_data: Bytes) -> Self {
        Self { extra_data }
    }
}

impl EthereumBuilderConfig {
    /// Returns owned extra data bytes for the block.
    pub fn extra_data(&self) -> Bytes {
        self.extra_data.clone()
    }
}
