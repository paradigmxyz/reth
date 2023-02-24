use reth_interfaces::Result;
use reth_primitives::{BlockId, Header};
use revm_primitives::{BlockEnv, CfgEnv, Env};

/// A provider type that knows chain specific information required to configure an [Env]
///
/// This type is mainly used to provide required data to configure the EVM environment.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider: Send + Sync {
    /// Fills the [CfgEnv] and [BlockEnv] fields with values specific to the given [BlockId].
    fn fill_env_at(&self, env: &mut Env, at: BlockId) -> Result<()>;

    /// Fills the [CfgEnv] and [BlockEnv]  fields with values specific to the given [Header].
    fn fill_env_with_header(&self, env: &mut Env, header: &Header) -> Result<()>;

    /// Fills the [BlockEnv] fields with values specific to the given [BlockId].
    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockId) -> Result<()>;

    /// Fills the [BlockEnv] fields with values specific to the given [Header].
    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()>;

    /// Fills the [CfgEnv] fields with values specific to the given [BlockId].
    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockId) -> Result<()>;

    /// Fills the [CfgEnv] fields with values specific to the given [Header].
    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()>;
}
