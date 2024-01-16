use reth_primitives::{
    revm::env::{fill_block_env_with_coinbase, recover_header_signer},
    Address, Chain, ChainSpec, Header, Transaction, U256,
};
use revm_primitives::{BlockEnv, CfgEnv, SpecId, TxEnv};

/// This represents the set of methods used to configure the EVM before execution.
pub trait EvmEnvConfig {
    /// Additional metadata that can be used when filling the transaction environment.
    type TxMeta;

    /// Fill transaction environment from a [Transaction] and the given sender address.
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>;

    /// Fill [CfgEnv] fields according to the chain spec and given header
    fn fill_cfg_env(
        cfg_env: &mut CfgEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    );

    /// Convenience function to call both [fill_cfg_env](EvmEnvConfig::fill_cfg_env) and
    /// [fill_block_env](EvmEnvConfig::fill_block_env).
    fn fill_cfg_and_block_env(
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        Self::fill_cfg_env(cfg, chain_spec, header, total_difficulty);
        let after_merge = cfg.spec_id >= SpecId::MERGE;
        Self::fill_block_env(block_env, chain_spec, header, after_merge);
    }

    /// Fill block environment from Block.
    fn fill_block_env(
        block_env: &mut BlockEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        after_merge: bool,
    ) {
        let coinbase = Self::block_coinbase(chain_spec, header, after_merge);
        fill_block_env_with_coinbase(block_env, header, after_merge, coinbase);
    }

    /// Return the coinbase address for the given header and chain spec.
    fn block_coinbase(chain_spec: &ChainSpec, header: &Header, after_merge: bool) -> Address {
        if chain_spec.chain == Chain::goerli() && !after_merge {
            recover_header_signer(header).unwrap_or_else(|err| {
                panic!(
                    "Failed to recover goerli Clique Consensus signer from header ({}, {}) using extradata {}: {:?}",
                    header.number, header.hash_slow(), header.extra_data, err
                )
            })
        } else {
            header.beneficiary
        }
    }
}
