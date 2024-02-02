use reth_interfaces::provider::ProviderResult;
use reth_node_api::EvmEnvConfig;
use reth_primitives::{BlockHashOrNumber, Header};
use revm::primitives::{BlockEnv, CfgEnv, CfgEnvWithSpecId, SpecId};

/// A provider type that knows chain specific information required to configure an
/// [Env](revm::primitives::Env).
///
/// This type is mainly used to provide required data to configure the EVM environment.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider: Send + Sync {
    /// Fills the [CfgEnvWithSpecId] and [BlockEnv] fields with values specific to the given
    /// [BlockHashOrNumber].
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithSpecId,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: EvmEnvConfig;

    /// Fills the default [CfgEnvWithSpecId] and [BlockEnv] fields with values specific to the given
    /// [Header].
    fn env_with_header<EvmConfig>(
        &self,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<(CfgEnvWithSpecId, BlockEnv)>
    where
        EvmConfig: EvmEnvConfig,
    {
        let mut cfg = CfgEnvWithSpecId::new(CfgEnv::default(), SpecId::LATEST);
        let mut block_env = BlockEnv::default();
        self.fill_env_with_header::<EvmConfig>(&mut cfg, &mut block_env, header, evm_config)?;
        Ok((cfg, block_env))
    }

    /// Fills the [CfgEnvWithSpecId] and [BlockEnv]  fields with values specific to the given
    /// [Header].
    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithSpecId,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: EvmEnvConfig;

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

    /// Fills the [CfgEnvWithSpecId] fields with values specific to the given [BlockHashOrNumber].
    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithSpecId,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: EvmEnvConfig;

    /// Fills the [CfgEnvWithSpecId] fields with values specific to the given [Header].
    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithSpecId,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: EvmEnvConfig;
}
