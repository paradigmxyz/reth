//! Provider trait for populating the EVM environment.

use crate::ConfigureEvmEnv;
use alloy_consensus::Header;
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};

/// A provider type that knows chain specific information required to configure a
/// [`CfgEnvWithHandlerCfg`].
///
/// This type is mainly used to provide required data to configure the EVM environment that is
/// not part of the block and stored separately (on disk), for example the total difficulty.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider<H = Header>: Send + Sync {
    /// Fills the default [`CfgEnvWithHandlerCfg`] and [BlockEnv] fields with values specific to the
    /// given block header.
    fn env_with_header<EvmConfig>(
        &self,
        header: &H,
        evm_config: EvmConfig,
    ) -> ProviderResult<(CfgEnvWithHandlerCfg, BlockEnv)>
    where
        EvmConfig: ConfigureEvmEnv<Header = H>;
}
