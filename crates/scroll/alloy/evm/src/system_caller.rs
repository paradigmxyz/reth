use alloc::string::ToString;

use alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS;
use alloy_evm::{
    block::{BlockExecutionError, BlockValidationError},
    Evm,
};
use alloy_primitives::B256;
use revm::{context::result::ResultAndState, DatabaseCommit};
use scroll_alloy_hardforks::ScrollHardforks;

/// An ephemeral helper type for executing system calls.
///
/// This can be used to chain system transaction calls.
#[derive(Debug)]
pub(crate) struct ScrollSystemCaller<Spec> {
    spec: Spec,
}

impl<Spec> ScrollSystemCaller<Spec> {
    /// Create a new system caller with the given spec.
    pub(crate) const fn new(spec: Spec) -> Self {
        Self { spec }
    }
}

impl<Spec> ScrollSystemCaller<Spec>
where
    Spec: ScrollHardforks,
{
    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub(crate) fn apply_blockhashes_contract_call(
        &self,
        parent_block_hash: B256,
        evm: &mut impl Evm<DB: DatabaseCommit>,
    ) -> Result<(), BlockExecutionError> {
        let result_and_state =
            transact_blockhashes_contract_call(&self.spec, parent_block_hash, evm)?;

        if let Some(res) = result_and_state {
            evm.db_mut().commit(res.state);
        }

        Ok(())
    }
}

/// Applies the pre-block call to the [EIP-2935] blockhashes contract, using the given block,
/// chain specification, and EVM.
///
/// If Feynman is not activated, or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// Returns `None` if Feynman is not active or the block is the genesis block, otherwise returns the
/// result of the call.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
#[inline]
fn transact_blockhashes_contract_call<Halt>(
    spec: impl ScrollHardforks,
    parent_block_hash: B256,
    evm: &mut impl Evm<HaltReason = Halt>,
) -> Result<Option<ResultAndState<Halt>>, BlockExecutionError> {
    // if Feynman is not active at timestamp then no system transaction occurs.
    if !spec.is_feynman_active_at_timestamp(evm.block().timestamp) {
        return Ok(None);
    }

    // if the block number is zero (genesis block) then no system transaction may occur as per
    // EIP-2935
    if evm.block().number == 0 {
        return Ok(None);
    }

    let res = match evm.transact_system_call(
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        HISTORY_STORAGE_ADDRESS,
        parent_block_hash.0.into(),
    ) {
        Ok(res) => res,
        Err(e) => {
            return Err(BlockValidationError::BlockHashContractCall { message: e.to_string() }.into())
        }
    };

    Ok(Some(res))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{convert::Infallible, sync::Arc};

    use crate::curie::L1_GAS_PRICE_ORACLE_ADDRESS;
    use alloy_consensus::{Block, BlockBody, Header};
    use alloy_eips::eip2935::HISTORY_STORAGE_CODE;
    use alloy_hardforks::ForkCondition;
    use alloy_primitives::{keccak256, U256};
    use reth_evm::ConfigureEvm;
    use reth_scroll_chainspec::{ScrollChainConfig, ScrollChainSpecBuilder};
    use reth_scroll_evm::ScrollEvmConfig;
    use revm::{
        bytecode::Bytecode,
        context::ContextTr,
        database::{EmptyDBTyped, State},
        state::AccountInfo,
        Database,
    };
    use scroll_alloy_consensus::ScrollTxEnvelope;
    use scroll_alloy_hardforks::{ScrollChainHardforks, ScrollHardfork};

    #[test]
    fn test_should_not_apply_blockhashes_contract_call_before_feynman() {
        // initiate system caller.
        let system_caller = ScrollSystemCaller::new(ScrollChainHardforks::new([
            (ScrollHardfork::EuclidV2, ForkCondition::Timestamp(0)),
            (ScrollHardfork::Feynman, ForkCondition::Timestamp(100)),
        ]));

        // initiate db with system contract.
        let db = EmptyDBTyped::<Infallible>::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        state.insert_account(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo {
                code_hash: keccak256(HISTORY_STORAGE_CODE.clone()),
                code: Some(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
                ..Default::default()
            },
        );

        // load l1 oracle in state.
        state.insert_account(L1_GAS_PRICE_ORACLE_ADDRESS, Default::default());

        // prepare chain spec.
        let chain_spec =
            Arc::new(ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()));
        let evm_config = ScrollEvmConfig::scroll(chain_spec);

        let header = Header {
            parent_hash: B256::random(),
            number: 1,
            gas_limit: 20_000_000,
            ..Default::default()
        };
        let block: Block<ScrollTxEnvelope, _> = Block { header, body: BlockBody::default() };

        // initiate the evm and apply the block hashes contract call.
        let mut evm = evm_config.evm_for_block(state, &block.header);
        system_caller.apply_blockhashes_contract_call(block.parent_hash, &mut evm).unwrap();

        // assert the storage slot remains unchanged.
        let parent_hash = evm.db().storage(HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap();
        assert_eq!(parent_hash, U256::ZERO);
    }

    #[test]
    fn test_should_apply_blockhashes_contract_call_after_feynman() {
        // initiate system caller.
        let system_caller = ScrollSystemCaller::new(ScrollChainHardforks::new([(
            ScrollHardfork::Feynman,
            ForkCondition::Timestamp(0),
        )]));

        // initiate db with system contract.
        let db = EmptyDBTyped::<Infallible>::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        state.insert_account(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo {
                code_hash: keccak256(HISTORY_STORAGE_CODE.clone()),
                code: Some(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
                ..Default::default()
            },
        );

        // load l1 oracle in state.
        state.insert_account(L1_GAS_PRICE_ORACLE_ADDRESS, Default::default());

        // prepare chain spec.
        let chain_spec =
            Arc::new(ScrollChainSpecBuilder::scroll_mainnet().build(ScrollChainConfig::mainnet()));
        let evm_config = ScrollEvmConfig::scroll(chain_spec);

        let header = Header {
            parent_hash: B256::random(),
            number: 1,
            gas_limit: 20_000_000,
            ..Default::default()
        };
        let block: Block<ScrollTxEnvelope, _> = Block { header, body: BlockBody::default() };

        // initiate the evm and apply the block hashes contract call.
        let mut evm = evm_config.evm_for_block(state, &block.header);
        system_caller.apply_blockhashes_contract_call(block.parent_hash, &mut evm).unwrap();

        // assert the hash is written to storage.
        let parent_hash = evm.db().storage(HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap();
        assert_eq!(Into::<B256>::into(parent_hash), block.parent_hash);
    }
}
