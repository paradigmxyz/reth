use super::evm::config::BscEvmConfig;
use crate::chainspec::BscChainSpec;
use reth_evm::execute::BasicBlockExecutorProvider;
use std::sync::Arc;

/// Helper type with backwards compatible methods to obtain executor providers.
#[derive(Debug)]
pub struct BscExecutorProvider;

impl BscExecutorProvider {
    /// Creates a new default optimism executor strategy factory.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(chain_spec: Arc<BscChainSpec>) -> BasicBlockExecutorProvider<BscEvmConfig> {
        BasicBlockExecutorProvider::new(BscEvmConfig::new(chain_spec))
    }
}
