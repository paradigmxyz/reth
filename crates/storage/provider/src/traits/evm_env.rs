use reth_interfaces::provider::ProviderResult;
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{BlockHashOrNumber, Header};
use revm::primitives::{BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, SpecId};

/// A provider type that knows chain specific information required to configure an
/// [CfgEnvWithHandlerCfg].
///
/// This type is mainly used to provide required data to configure the EVM environment.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider: Send + Sync {
    /// Fills the [CfgEnvWithHandlerCfg] and [BlockEnv] fields with values specific to the given
    /// [BlockHashOrNumber].
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv;

    /// Fills the default [CfgEnvWithHandlerCfg] and [BlockEnv] fields with values specific to the
    /// given [Header].
    fn env_with_header<EvmConfig>(
        &self,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<(CfgEnvWithHandlerCfg, BlockEnv)>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        let mut cfg = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);
        let mut block_env = BlockEnv::default();
        self.fill_env_with_header::<EvmConfig>(&mut cfg, &mut block_env, header, evm_config)?;
        Ok((cfg, block_env))
    }

    /// Fills the [CfgEnvWithHandlerCfg] and [BlockEnv]  fields with values specific to the given
    /// [Header].
    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv;

    /// Fills the [BlockEnv] fields with values specific to the given [BlockHashOrNumber].
    fn fill_block_env_at(
        &self,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> ProviderResult<()>;

    /// Fills the [BlockEnv] fields with values specific to the given [Header].
    fn fill_block_env_with_header(
        &self,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> ProviderResult<()>;

    /// Fills the [CfgEnvWithHandlerCfg] fields with values specific to the given
    /// [BlockHashOrNumber].
    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv;

    /// Fills the [CfgEnvWithHandlerCfg] fields with values specific to the given [Header].
    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv;
}
