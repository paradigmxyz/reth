//! Provider trait for populating the EVM environment.

use crate::ConfigureEvmEnv;
use alloy_eips::eip1898::BlockHashOrNumber;
use reth_primitives::Header;
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, SpecId};

/// A provider type that knows chain specific information required to configure a
/// [`CfgEnvWithHandlerCfg`].
///
/// This type is mainly used to provide required data to configure the EVM environment that is
/// usually stored on disk.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider: Send + Sync {
    /// Fills the [`CfgEnvWithHandlerCfg`] and [BlockEnv] fields with values specific to the given
    /// [BlockHashOrNumber].
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>;

    /// Fills the default [`CfgEnvWithHandlerCfg`] and [BlockEnv] fields with values specific to the
    /// given [Header].
    fn env_with_header<EvmConfig>(
        &self,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<(CfgEnvWithHandlerCfg, BlockEnv)>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let mut cfg = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);
        let mut block_env = BlockEnv::default();
        self.fill_env_with_header(&mut cfg, &mut block_env, header, evm_config)?;
        Ok((cfg, block_env))
    }

    /// Fills the [`CfgEnvWithHandlerCfg`] and [BlockEnv]  fields with values specific to the given
    /// [Header].
    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>;

    /// Fills the [`CfgEnvWithHandlerCfg`] fields with values specific to the given
    /// [BlockHashOrNumber].
    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>;

    /// Fills the [`CfgEnvWithHandlerCfg`] fields with values specific to the given [Header].
    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>;
}
