use alloc::sync::Arc;

/// Block assembler placeholder for Ethereum.
///
/// The previous legacy assembler is parked while payload building is ported to the active EVM.
#[derive(Debug, Clone)]
pub struct EthBlockAssembler<ChainSpec = reth_chainspec::ChainSpec> {
    /// The chainspec.
    pub chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> EthBlockAssembler<ChainSpec> {
    /// Creates a new [`EthBlockAssembler`].
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}
