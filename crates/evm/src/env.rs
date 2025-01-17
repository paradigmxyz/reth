use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};

/// Container type that holds both the configuration and block environment for EVM execution.
#[derive(Debug, Clone)]
pub struct EvmEnv {
    /// The configuration environment with handler settings
    pub cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg,
    /// The block environment containing block-specific data
    pub block_env: BlockEnv,
}

impl Default for EvmEnv {
    fn default() -> Self {
        Self {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                cfg_env: Default::default(),
                // Will set `is_optimism` if `revm/optimism-default-handler` is enabled.
                handler_cfg: Default::default(),
            },
            block_env: BlockEnv::default(),
        }
    }
}

impl EvmEnv {
    /// Create a new `EvmEnv` from its components.
    ///
    /// # Arguments
    ///
    /// * `cfg_env_with_handler_cfg` - The configuration environment with handler settings
    /// * `block` - The block environment containing block-specific data
    pub const fn new(cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg, block_env: BlockEnv) -> Self {
        Self { cfg_env_with_handler_cfg, block_env }
    }

    /// Returns a reference to the block environment.
    pub const fn block_env(&self) -> &BlockEnv {
        &self.block_env
    }

    /// Returns a reference to the configuration environment.
    pub const fn cfg_env_with_handler_cfg(&self) -> &CfgEnvWithHandlerCfg {
        &self.cfg_env_with_handler_cfg
    }
}

impl From<(CfgEnvWithHandlerCfg, BlockEnv)> for EvmEnv {
    fn from((cfg_env_with_handler_cfg, block_env): (CfgEnvWithHandlerCfg, BlockEnv)) -> Self {
        Self { cfg_env_with_handler_cfg, block_env }
    }
}

impl From<EvmEnv> for (CfgEnvWithHandlerCfg, BlockEnv) {
    fn from(env: EvmEnv) -> Self {
        (env.cfg_env_with_handler_cfg, env.block_env)
    }
}
