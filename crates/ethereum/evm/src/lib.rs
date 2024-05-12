//! EVM config for vanilla ethereum.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![allow(unused_crate_dependencies)]
#![allow(unreachable_pub)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{
    revm::{config::revm_spec, env::fill_tx_env},
    revm_primitives::{AnalysisKind, CfgEnvWithHandlerCfg, TxEnv},
    Address, ChainSpec, Head, Header, TransactionSigned, U256,
};
use reth_revm::{Database, EvmBuilder};
pub mod execute;
pub mod verify;

/// Ethereum DAO hardfork state change data.
pub mod dao_fork;

mod instructions;

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct EthEvmConfig;

impl ConfigureEvmEnv for EthEvmConfig {
    fn fill_tx_env(tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
        fill_tx_env(tx_env, transaction, sender)
    }

    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        let spec_id = revm_spec(
            chain_spec,
            Head {
                number: header.number,
                timestamp: header.timestamp,
                difficulty: header.difficulty,
                total_difficulty,
                hash: Default::default(),
            },
        );

        cfg_env.chain_id = chain_spec.chain().id();
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

        cfg_env.handler_cfg.spec_id = spec_id;
    }
}

use instructions::{context::InstructionsContext, eip3074, BoxedInstructionWithOpCode};
use revm_interpreter::{opcode::InstructionTables, Host};
use std::sync::Arc;

/// Inserts the given boxed instructions with opcodes in the instructions table.
fn insert_boxed_instructions<'a, I, H>(
    table: &mut InstructionTables<'a, H>,
    boxed_instructions_with_opcodes: I,
) where
    I: Iterator<Item = BoxedInstructionWithOpCode<'a, H>>,
    H: Host + 'a,
{
    for boxed_instruction_with_opcode in boxed_instructions_with_opcodes {
        table.insert_boxed(
            boxed_instruction_with_opcode.opcode,
            boxed_instruction_with_opcode.boxed_instruction,
        );
    }
}

impl ConfigureEvm for EthEvmConfig {
    type DefaultExternalContext<'a> = ();

    fn evm<'a, DB: Database + 'a>(
        &self,
        db: DB,
    ) -> reth_revm::Evm<'a, Self::DefaultExternalContext<'a>, DB> {
        let instructions_context = InstructionsContext::default();
        EvmBuilder::default()
            .with_db(db)
            .append_handler_register_box(Box::new(move |h| {
                if let Some(ref mut table) = h.instruction_table {
                    insert_boxed_instructions(
                        table,
                        eip3074::boxed_instructions(instructions_context.clone()),
                    );

                    instructions_context.clear();
                }

                let post_execution_context = instructions_context.clone();
                #[allow(clippy::arc_with_non_send_sync)]
                {
                    h.post_execution.end = Arc::new(move |_, outcome: _| {
                        post_execution_context.clear();
                        outcome
                    });
                }
            }))
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::revm_primitives::{BlockEnv, CfgEnv, SpecId};

    #[test]
    #[ignore]
    fn test_fill_cfg_and_block_env() {
        let mut cfg_env = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);
        let mut block_env = BlockEnv::default();
        let header = Header::default();
        let chain_spec = ChainSpec::default();
        let total_difficulty = U256::ZERO;

        EthEvmConfig::fill_cfg_and_block_env(
            &mut cfg_env,
            &mut block_env,
            &chain_spec,
            &header,
            total_difficulty,
        );

        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }
}
