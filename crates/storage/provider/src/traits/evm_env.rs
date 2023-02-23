use reth_primitives::{BlockId, Header};
use revm_primitives::{BlockEnv, CfgEnv, Env};

/// Error that can happen when trying to fill the Env
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum EvmEnvProviderError {
    /// Error when interacting with the DB
    #[error(transparent)]
    Database(#[from] reth_interfaces::db::Error),
    /// Thrown when a DB lookup did not find required data, for example when the requested header
    /// does not exist.
    #[error("requested data not found")]
    HeaderNotFound,
}

/// A provider type that knows chain specific information required to configure an [Env]
///
/// This type is mainly used to provide required data to configure the EVM environment.
#[auto_impl::auto_impl(&, Arc)]
pub trait EvmEnvProvider: Send + Sync {
    /// Fills the [CfgEnv] and [BlockEnv] fields with values specific to the given [BlockId].
    fn fill_env_at(&self, env: &mut Env, at: BlockId) -> Result<(), EvmEnvProviderError>;

    /// Fills the [CfgEnv] and [BlockEnv]  fields with values specific to the given [Header].
    fn fill_env_with_header(
        &self,
        env: &mut Env,
        header: &Header,
    ) -> Result<(), EvmEnvProviderError>;

    /// Fills the [BlockEnv] fields with values specific to the given [BlockId].
    fn fill_block_env_at(&self, env: &mut BlockEnv, at: BlockId)
        -> Result<(), EvmEnvProviderError>;

    /// Fills the [BlockEnv] fields with values specific to the given [Heaver].
    fn fill_block_env_with_header(
        &self,
        env: &mut BlockEnv,
        header: &Header,
        after_merge: bool,
    ) -> Result<(), EvmEnvProviderError>;

    /// Fills the [CfgEnv] fields with values specific to the given [BlockId].
    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockId) -> Result<(), EvmEnvProviderError>;

    /// Fills the [CfgEnv] fields with values specific to the given [Header].
    fn fill_cfg_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        header: &Header,
    ) -> Result<(), EvmEnvProviderError>;
}
