use crate::{
    in_memory::ExecutedBlock, CanonStateNotification, CanonStateNotifications,
    CanonStateSubscriptions,
};
use rand::{thread_rng, Rng};
use reth_chainspec::ChainSpec;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::{
    constants::{EIP1559_INITIAL_BASE_FEE, EMPTY_ROOT_HASH},
    proofs::{calculate_receipt_root, calculate_transaction_root},
    Address, BlockNumber, Header, Receipt, Receipts, Requests, SealedBlock, SealedBlockWithSenders,
    Signature, Transaction, TransactionSigned, TransactionSignedEcRecovered, TxEip1559, B256, U256,
};
use reth_trie::{root::state_root_unhashed, updates::TrieUpdates, HashedPostState};
use revm::{db::BundleState, primitives::AccountInfo};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast::{self, Sender};

/// Generates a random `SealedBlockWithSenders`.
pub fn generate_random_block(
    number: BlockNumber,
    parent_hash: B256,
    chain_spec: &ChainSpec,
    signer: Address,
    signer_balance: &mut U256,
    nonce: &mut u64,
) -> SealedBlockWithSenders {
    let mut rng = thread_rng();

    let single_tx_cost = U256::from(EIP1559_INITIAL_BASE_FEE * 21_000);
    let mock_tx = |nonce: u64| -> TransactionSignedEcRecovered {
        TransactionSigned::from_transaction_and_signature(
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce,
                gas_limit: 21_000,
                to: Address::random().into(),
                max_fee_per_gas: EIP1559_INITIAL_BASE_FEE as u128,
                max_priority_fee_per_gas: 1,
                ..Default::default()
            }),
            Signature::default(),
        )
        .with_signer(signer)
    };

    let num_txs = rng.gen_range(0..5);
    let signer_balance_decrease = single_tx_cost * U256::from(num_txs);
    let transactions: Vec<TransactionSignedEcRecovered> = (0..num_txs)
        .map(|_| {
            let tx = mock_tx(*nonce);
            *nonce += 1;
            *signer_balance -= signer_balance_decrease;
            tx
        })
        .collect();

    let receipts = transactions
        .iter()
        .enumerate()
        .map(|(idx, tx)| {
            Receipt {
                tx_type: tx.tx_type(),
                success: true,
                cumulative_gas_used: (idx as u64 + 1) * 21_000,
                ..Default::default()
            }
            .with_bloom()
        })
        .collect::<Vec<_>>();

    let initial_signer_balance = U256::from(10).pow(U256::from(18));

    let header = Header {
        number,
        parent_hash,
        gas_used: transactions.len() as u64 * 21_000,
        gas_limit: chain_spec.max_gas_limit,
        mix_hash: B256::random(),
        base_fee_per_gas: Some(EIP1559_INITIAL_BASE_FEE),
        transactions_root: calculate_transaction_root(&transactions),
        receipts_root: calculate_receipt_root(&receipts),
        beneficiary: Address::random(),
        state_root: state_root_unhashed(HashMap::from([(
            signer,
            (
                AccountInfo {
                    balance: initial_signer_balance - signer_balance_decrease,
                    nonce: num_txs,
                    ..Default::default()
                },
                EMPTY_ROOT_HASH,
            ),
        )])),
        ..Default::default()
    };

    let block = SealedBlock {
        header: header.seal_slow(),
        body: transactions.into_iter().map(|tx| tx.into_signed()).collect(),
        ommers: Vec::new(),
        withdrawals: None,
        requests: None,
    };

    SealedBlockWithSenders::new(block, vec![signer; num_txs as usize]).unwrap()
}

/// Creates a fork chain with the given base block.
pub fn create_fork(
    base_block: &SealedBlock,
    length: u64,
    chain_spec: &ChainSpec,
    signer: Address,
    initial_signer_balance: U256,
) -> Vec<SealedBlockWithSenders> {
    let mut fork = Vec::with_capacity(length as usize);
    let mut parent = base_block.clone();
    let mut signer_balance = initial_signer_balance;
    let mut nonce = 0;

    for _ in 0..length {
        let block = generate_random_block(
            parent.number + 1,
            parent.hash(),
            chain_spec,
            signer,
            &mut signer_balance,
            &mut nonce,
        );
        parent = block.block.clone();
        fork.push(block);
    }

    fork
}

fn get_executed_block(
    block_number: BlockNumber,
    receipts: Receipts,
    parent_hash: B256,
) -> ExecutedBlock {
    let chain_spec = ChainSpec::default();
    let signer = Address::random();
    let mut signer_balance = U256::from(1_000_000_000_000_000_000u64);
    let mut nonce = 0;

    let block_with_senders = generate_random_block(
        block_number,
        parent_hash,
        &chain_spec,
        signer,
        &mut signer_balance,
        &mut nonce,
    );

    ExecutedBlock::new(
        Arc::new(block_with_senders.block.clone()),
        Arc::new(block_with_senders.senders),
        Arc::new(ExecutionOutcome::new(
            BundleState::default(),
            receipts,
            block_number,
            vec![Requests::default()],
        )),
        Arc::new(HashedPostState::default()),
        Arc::new(TrieUpdates::default()),
    )
}

/// Generates an `ExecutedBlock` that includes the given `Receipts`.
pub fn get_executed_block_with_receipts(receipts: Receipts, parent_hash: B256) -> ExecutedBlock {
    let number = rand::thread_rng().gen::<u64>();
    get_executed_block(number, receipts, parent_hash)
}

/// Generates an `ExecutedBlock` with the given `BlockNumber`.
pub fn get_executed_block_with_number(
    block_number: BlockNumber,
    parent_hash: B256,
) -> ExecutedBlock {
    get_executed_block(block_number, Receipts { receipt_vec: vec![vec![]] }, parent_hash)
}

/// Generates a range of executed blocks with ascending block numbers.
pub fn get_executed_blocks(range: Range<u64>) -> impl Iterator<Item = ExecutedBlock> {
    let mut parent_hash = B256::default();
    range.map(move |number| {
        let current_parent_hash = parent_hash;
        let block = get_executed_block_with_number(number, current_parent_hash);
        parent_hash = block.block.hash();
        block
    })
}

/// A test `ChainEventSubscriptions`
#[derive(Clone, Debug, Default)]
pub struct TestCanonStateSubscriptions {
    canon_notif_tx: Arc<Mutex<Vec<Sender<CanonStateNotification>>>>,
}

impl TestCanonStateSubscriptions {
    /// Adds new block commit to the queue that can be consumed with
    /// [`TestCanonStateSubscriptions::subscribe_to_canonical_state`]
    pub fn add_next_commit(&self, new: Arc<Chain>) {
        let event = CanonStateNotification::Commit { new };
        self.canon_notif_tx.lock().as_mut().unwrap().retain(|tx| tx.send(event.clone()).is_ok())
    }

    /// Adds reorg to the queue that can be consumed with
    /// [`TestCanonStateSubscriptions::subscribe_to_canonical_state`]
    pub fn add_next_reorg(&self, old: Arc<Chain>, new: Arc<Chain>) {
        let event = CanonStateNotification::Reorg { old, new };
        self.canon_notif_tx.lock().as_mut().unwrap().retain(|tx| tx.send(event.clone()).is_ok())
    }
}

impl CanonStateSubscriptions for TestCanonStateSubscriptions {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        let (canon_notif_tx, canon_notif_rx) = broadcast::channel(100);
        self.canon_notif_tx.lock().as_mut().unwrap().push(canon_notif_tx);

        canon_notif_rx
    }
}
