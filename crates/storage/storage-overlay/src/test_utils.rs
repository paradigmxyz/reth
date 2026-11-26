//! Helpers shared by the tests of this crate.

use crate::{OverlayContext, StateTrieOverlay};
use alloy_primitives::{map::HashMap, Address, B256, U256};
use evm2::{bytecode::Bytecode, evm::AccountInfo};
use reth_chain_state::{
    test_utils::TestBlockBuilder, ExecutedBlock, InMemoryState, InMemoryStateWriter,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::Account;
use reth_provider::{
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    BlockWriter, ProviderFactory,
};
use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
use reth_storage_api::StageCheckpointWriter;
use reth_trie::{
    updates::TrieUpdatesSorted, BranchNodeCompact, HashedPostState, HashedStorage, Nibbles,
    SortedTrieData,
};
use reth_trie_common::bundle_state::BundleState;
use std::sync::Arc;

/// Creates a fresh overlay context and the writer for its in-memory state.
pub(crate) fn overlay_context_with_writer() -> (OverlayContext, InMemoryStateWriter) {
    let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
    (OverlayContext::new(in_memory_state), in_memory_state_writer)
}

/// Asserts that `actual` holds the same hashed accounts and account trie nodes as `expected`,
/// which must hold account trie nodes, so that the comparison covers them.
pub(crate) fn assert_same_overlay(actual: &StateTrieOverlay, expected: &StateTrieOverlay) {
    let (actual, expected) = (actual.input(), expected.input());
    assert!(
        !expected.nodes.account_nodes_ref().is_empty(),
        "nothing to compare account nodes with"
    );
    assert_eq!(actual.state.accounts, expected.state.accounts);
    assert_eq!(actual.nodes.account_nodes_ref(), expected.nodes.account_nodes_ref());
}

pub(crate) fn with_unique_trie_data(
    block: &ExecutedBlock<EthPrimitives>,
    id: u8,
) -> ExecutedBlock<EthPrimitives> {
    let hashed_address = B256::with_last_byte(id);
    let hashed_slot = B256::with_last_byte(id.saturating_add(32));
    let hashed_state = HashedPostState::default()
        .with_accounts([(hashed_address, Some(Account::default()))])
        .with_storages([(
            hashed_address,
            HashedStorage::from_iter([(hashed_slot, U256::from(id))]),
        )])
        .into_sorted();
    let trie_updates = TrieUpdatesSorted::new(
        vec![(Nibbles::from_nibbles([id]), Some(BranchNodeCompact::new(0, 0, 0, vec![], None)))],
        Default::default(),
    );
    let address = Address::with_last_byte(id);
    let slot = U256::from(id);
    let code_hash = B256::with_last_byte(id.saturating_add(64));
    let state = BundleState::builder(block.block_number()..=block.block_number())
        .state_present_account_info(
            address,
            AccountInfo { nonce: id as u64, balance: U256::from(id), ..Default::default() },
        )
        .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, U256::from(id)))]))
        .contract(code_hash, Bytecode::new_raw(vec![id].into()))
        .build();
    let mut execution_output = (*block.execution_output).clone();
    execution_output.state = state;

    ExecutedBlock::new(
        Arc::clone(&block.recovered_block),
        Arc::new(execution_output),
        SortedTrieData::new(Arc::new(hashed_state), Arc::new(trie_updates)),
    )
}

pub(crate) fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
    TestBlockBuilder::eth()
        .get_executed_blocks(0..5)
        .enumerate()
        .map(|(index, block)| with_unique_trie_data(&block, index as u8 + 1))
        .collect()
}

pub(crate) fn setup_frontiers(
    state_trie_tip_index: usize,
    finish_tip_index: usize,
) -> (ProviderFactory<MockNodeTypesWithDB>, Vec<ExecutedBlock<EthPrimitives>>) {
    let factory = create_test_provider_factory();
    let blocks = test_blocks();
    let provider_rw = factory.provider_rw().unwrap();
    for block in &blocks[..=finish_tip_index] {
        provider_rw.insert_block(block.recovered_block()).unwrap();
    }
    provider_rw
        .save_stage_checkpoint(
            StageId::Finish,
            StageCheckpoint::new(blocks[finish_tip_index].block_number())
                .with_finish_stage_checkpoint(FinishCheckpoint {
                    partial_state_trie: Some(blocks[state_trie_tip_index].block_number()),
                }),
        )
        .unwrap();
    provider_rw.commit().unwrap();

    (factory, blocks)
}

pub(crate) fn account_keys(overlay: &StateTrieOverlay) -> Vec<B256> {
    overlay.input().state.accounts.iter().map(|(key, _)| *key).collect()
}

pub(crate) fn account_node_paths(overlay: &StateTrieOverlay) -> Vec<Nibbles> {
    overlay.input().nodes.account_nodes_ref().iter().map(|(path, _)| *path).collect()
}
