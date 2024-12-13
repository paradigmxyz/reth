//! Provider trait for populating the EVM environment.

use crate::{env::EvmEnv, ConfigureEvmEnv};
use alloy_consensus::Header;
use reth_storage_errors::provider::ProviderResult;

/// A provider type that knows chain specific information required to configure a
/// [`EvmEnv`].
///
/// This type is mainly used to provide required data to configure the EVM environment that is
/// not part of the block and stored separately (on disk), for example the total difficulty.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider<H = Header>: Send + Sync {
    /// Fills the default [`EvmEnv`] fields with values specific to the
    /// given block header.
    fn env_with_header<EvmConfig>(
        &self,
        header: &H,
        evm_config: EvmConfig,
    ) -> ProviderResult<EvmEnv>
    where
        EvmConfig: ConfigureEvmEnv<Header = H>;
}
