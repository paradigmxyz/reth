use reth_primitives::{
    revm::{config::revm_spec_by_timestamp_after_merge, env::fill_block_env},
    Address, ChainSpec, Header, Transaction, U256,
};
use revm_primitives::{BlockEnv, CfgEnvWithSpecId, SpecId, TxEnv};

/// This represents the set of methods used to configure the EVM before execution.
pub trait EvmEnvConfig: Send + Sync + Unpin + Clone {
    /// The type of the transaction metadata.
    type TxMeta;

    /// Fill transaction environment from a [Transaction] and the given sender address.
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>;

    /// Fill [CfgEnvWithSpecId] fields according to the chain spec and given header
    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithSpecId,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    );

    /// Convenience function to call both [fill_cfg_env](EvmEnvConfig::fill_cfg_env) and
    /// [fill_block_env].
    fn fill_cfg_and_block_env(
        cfg: &mut CfgEnvWithSpecId,
        block_env: &mut BlockEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        let spec_id = revm_spec_by_timestamp_after_merge(
            chain_spec,
            block_env.timestamp.try_into().unwrap_or(u64::MAX),
        );
        Self::fill_cfg_env(cfg, chain_spec, header, total_difficulty);
        let after_merge = spec_id >= SpecId::MERGE;
        fill_block_env(block_env, chain_spec, header, after_merge);
    }
}
