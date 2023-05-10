use reth_interfaces::Result;
use reth_primitives::{BlockHashOrNumber, Header};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};

/// A provider type that knows chain specific information required to configure an
/// [Env](reth_revm_primitives::primitives::Env)
///
/// This type is mainly used to provide required data to configure the EVM environment.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider: Send + Sync {
    /// Fills the [CfgEnv] and [BlockEnv] fields with values specific to the given
    /// [BlockHashOrNumber].
    fn fill_env_at(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> Result<()>;

    /// Fills the default [CfgEnv] and [BlockEnv] fields with values specific to the given [Header].
    fn env_with_header(&self, header: &Header) -> Result<(CfgEnv, BlockEnv)> {
        let mut cfg = CfgEnv::default();
        let mut block_env = BlockEnv::default();
        self.fill_env_with_header(&mut cfg, &mut block_env, header)?;
        Ok((cfg, block_env))
    }

    /// Fills the [CfgEnv] and [BlockEnv]  fields with values specific to the given [Header].
    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()>;

    /// Fills the [BlockEnv] fields with values specific to the given [BlockHashOrNumber].
    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockHashOrNumber) -> Result<()>;

    /// Fills the [BlockEnv] fields with values specific to the given [Header].
    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()>;

    /// Fills the [CfgEnv] fields with values specific to the given [BlockHashOrNumber].
    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockHashOrNumber) -> Result<()>;

    /// Fills the [CfgEnv] fields with values specific to the given [Header].
    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()>;
}
