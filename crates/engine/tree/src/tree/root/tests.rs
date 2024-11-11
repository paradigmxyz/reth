use super::{
    task::{StateRootTask, StdReceiverStream},
    StateRootConfig,
};
use reth_provider::{providers::ConsistentDbView, test_utils::MockEthProvider};
use reth_trie::TrieInput;
use revm_primitives::{
    Account, AccountInfo, AccountStatus, Address, EvmState, EvmStorage, EvmStorageSlot, HashMap,
    B256, U256,
};
use std::sync::Arc;

fn create_mock_config() -> StateRootConfig<MockEthProvider> {
    let factory = MockEthProvider::default();
    let view = ConsistentDbView::new(factory, None);
    let input = Arc::new(TrieInput::default());
    StateRootConfig { consistent_view: view, input }
}

fn create_mock_state() -> revm_primitives::EvmState {
    let mut state_changes: EvmState = HashMap::default();
    let storage = EvmStorage::from_iter([(U256::from(1), EvmStorageSlot::new(U256::from(2)))]);
    let account = Account {
        info: AccountInfo {
            balance: U256::from(100),
            nonce: 10,
            code_hash: B256::random(),
            code: Default::default(),
        },
        storage,
        status: AccountStatus::Loaded,
    };

    let address = Address::random();
    state_changes.insert(address, account);

    state_changes
}

#[test]
fn test_state_root_task() {
    let config = create_mock_config();
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = StdReceiverStream::new(rx);

    let task = StateRootTask::new(config, stream);
    let handle = task.spawn();

    for _ in 0..10 {
        tx.send(create_mock_state()).expect("failed to send state");
    }
    drop(tx);

    let result = handle.wait_for_result();
    assert!(result.is_ok(), "sync block execution failed");
}
