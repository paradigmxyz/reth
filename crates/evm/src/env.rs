use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use revm_primitives::{CfgEnv, SpecId};

/// Container type that holds both the configuration and block environment for EVM execution.
#[derive(Debug, Clone, Default)]
pub struct EvmEnv<Spec = SpecId> {
    /// The configuration environment with handler settings
    pub cfg_env: CfgEnv,
    /// The block environment containing block-specific data
    pub block_env: BlockEnv,
    /// The spec id of the chain. Specifies which hardfork is currently active, `Spec` type will
    /// most likely be an enum over hardforks.
    pub spec: Spec,
}

impl<Spec> EvmEnv<Spec> {
    /// Create a new `EvmEnv` from its components.
    ///
    /// # Arguments
    ///
    /// * `cfg_env_with_handler_cfg` - The configuration environment with handler settings
    /// * `block` - The block environment containing block-specific data
    pub const fn new(cfg_env: CfgEnv, block_env: BlockEnv, spec: Spec) -> Self {
        Self { cfg_env, spec, block_env }
    }

    /// Returns a reference to the block environment.
    pub const fn block_env(&self) -> &BlockEnv {
        &self.block_env
    }

    /// Returns a reference to the configuration environment.
    pub const fn cfg_env(&self) -> &CfgEnv {
        &self.cfg_env
    }

    /// Returns the spec id of the chain
    pub const fn spec_id(&self) -> &Spec {
        &self.spec
    }
}

impl From<(CfgEnvWithHandlerCfg, BlockEnv)> for EvmEnv {
    fn from((cfg_env_with_handler_cfg, block_env): (CfgEnvWithHandlerCfg, BlockEnv)) -> Self {
        let CfgEnvWithHandlerCfg { cfg_env, handler_cfg } = cfg_env_with_handler_cfg;
        Self { cfg_env, spec: handler_cfg.spec_id, block_env }
    }
}
