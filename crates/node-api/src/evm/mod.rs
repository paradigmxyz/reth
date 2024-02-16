use reth_primitives::{revm::env::fill_block_env, Address, ChainSpec, Header, Transaction, U256};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, SpecId, TxEnv};

/// EVM configuration trait.
pub trait EvmConfig: ConfigureEvmEnv + Clone + Send + Sync + 'static {}

/// This represents the set of methods used to configure the EVM before execution.
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone {
    /// The type of the transaction metadata.
    type TxMeta;

    /// Fill transaction environment from a [Transaction] and the given sender address.
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>;

    /// Fill [CfgEnvWithHandlerCfg] fields according to the chain spec and given header
    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    );

    /// Convenience function to call both [fill_cfg_env](ConfigureEvmEnv::fill_cfg_env) and
    /// [fill_block_env].
    fn fill_cfg_and_block_env(
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        Self::fill_cfg_env(cfg, chain_spec, header, total_difficulty);
        let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
        fill_block_env(block_env, chain_spec, header, after_merge);
    }
}
