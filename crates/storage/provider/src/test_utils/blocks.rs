//! Dummy blocks and data for tests
use crate::{DatabaseProviderRW, ExecutionOutcome};
use alloy_primitives::Log;
use alloy_rlp::Decodable;
use reth_db::tables;
use reth_db_api::{database::Database, models::StoredBlockBodyIndices};
use reth_primitives::{
    alloy_primitives, b256, hex_literal::hex, Account, Address, BlockNumber, Bytes, Header,
    Receipt, Requests, SealedBlock, SealedBlockWithSenders, TxType, Withdrawal, Withdrawals, B256,
    U256,
};
use reth_trie::root::{state_root_unhashed, storage_root_unhashed};
use revm::{
    db::BundleState,
    primitives::{AccountInfo, HashMap},
};

/// Assert genesis block
pub fn assert_genesis_block<DB: Database>(provider: &DatabaseProviderRW<DB>, g: SealedBlock) {
    let n = g.number;
    let h = B256::ZERO;
    let tx = provider;

    // check if all tables are empty
    assert_eq!(tx.table::<tables::Headers>().unwrap(), vec![(g.number, g.header.clone().unseal())]);

    assert_eq!(tx.table::<tables::HeaderNumbers>().unwrap(), vec![(h, n)]);
    assert_eq!(tx.table::<tables::CanonicalHeaders>().unwrap(), vec![(n, h)]);
    assert_eq!(
        tx.table::<tables::HeaderTerminalDifficulties>().unwrap(),
        vec![(n, g.difficulty.into())]
    );
    assert_eq!(
        tx.table::<tables::BlockBodyIndices>().unwrap(),
        vec![(0, StoredBlockBodyIndices::default())]
    );
    assert_eq!(tx.table::<tables::BlockOmmers>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::BlockWithdrawals>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::BlockRequests>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::Transactions>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::TransactionBlocks>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::TransactionHashNumbers>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::Receipts>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::PlainAccountState>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::PlainStorageState>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::AccountsHistory>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::StoragesHistory>().unwrap(), vec![]);
    // TODO check after this gets done: https://github.com/paradigmxyz/reth/issues/1588
    // Bytecodes are not reverted assert_eq!(tx.table::<tables::Bytecodes>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::AccountChangeSets>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::StorageChangeSets>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::HashedAccounts>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::HashedStorages>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::AccountsTrie>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::StoragesTrie>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::TransactionSenders>().unwrap(), vec![]);
    // StageCheckpoints is not updated in tests
}

const BLOCK_RLP: [u8; 610] = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0");

/// Test chain with genesis, blocks, execution results
/// that have valid changesets.
#[derive(Debug)]
pub struct BlockchainTestData {
    /// Genesis
    pub genesis: SealedBlock,
    /// Blocks with its execution result
    pub blocks: Vec<(SealedBlockWithSenders, ExecutionOutcome)>,
}

impl BlockchainTestData {
    /// Create test data with two blocks that are connected, specifying their block numbers.
    pub fn default_from_number(first: BlockNumber) -> Self {
        let one = block1(first);
        let mut extended_execution_outcome = one.1.clone();
        let two = block2(first + 1, one.0.hash(), &extended_execution_outcome);
        extended_execution_outcome.extend(two.1.clone());
        let three = block3(first + 2, two.0.hash(), &extended_execution_outcome);
        extended_execution_outcome.extend(three.1.clone());
        let four = block4(first + 3, three.0.hash(), &extended_execution_outcome);
        extended_execution_outcome.extend(four.1.clone());
        let five = block5(first + 4, four.0.hash(), &extended_execution_outcome);
        Self { genesis: genesis(), blocks: vec![one, two, three, four, five] }
    }
}

impl Default for BlockchainTestData {
    fn default() -> Self {
        let one = block1(1);
        let mut extended_execution_outcome = one.1.clone();
        let two = block2(2, one.0.hash(), &extended_execution_outcome);
        extended_execution_outcome.extend(two.1.clone());
        let three = block3(3, two.0.hash(), &extended_execution_outcome);
        extended_execution_outcome.extend(three.1.clone());
        let four = block4(4, three.0.hash(), &extended_execution_outcome);
        extended_execution_outcome.extend(four.1.clone());
        let five = block5(5, four.0.hash(), &extended_execution_outcome);
        Self { genesis: genesis(), blocks: vec![one, two, three, four, five] }
    }
}

/// Genesis block
pub fn genesis() -> SealedBlock {
    SealedBlock {
        header: Header { number: 0, difficulty: U256::from(1), ..Default::default() }
            .seal(B256::ZERO),
        body: vec![],
        ommers: vec![],
        withdrawals: Some(Withdrawals::default()),
        requests: Some(Requests::default()),
    }
}

fn bundle_state_root(execution_outcome: &ExecutionOutcome) -> B256 {
    state_root_unhashed(execution_outcome.bundle_accounts_iter().filter_map(
        |(address, account)| {
            account.info.as_ref().map(|info| {
                (
                    address,
                    (
                        Into::<Account>::into(info.clone()),
                        storage_root_unhashed(
                            account
                                .storage
                                .iter()
                                .filter(|(_, value)| !value.present_value.is_zero())
                                .map(|(slot, value)| ((*slot).into(), value.present_value)),
                        ),
                    ),
                )
            })
        },
    ))
}

/// Block one that points to genesis
fn block1(number: BlockNumber) -> (SealedBlockWithSenders, ExecutionOutcome) {
    // block changes
    let account1: Address = [0x60; 20].into();
    let account2: Address = [0x61; 20].into();
    let slot = U256::from(5);
    let info = AccountInfo { nonce: 1, balance: U256::from(10), ..Default::default() };

    let execution_outcome = ExecutionOutcome::new(
        BundleState::builder(number..=number)
            .state_present_account_info(account1, info.clone())
            .revert_account_info(number, account1, Some(None))
            .state_present_account_info(account2, info)
            .revert_account_info(number, account2, Some(None))
            .state_storage(account1, HashMap::from([(slot, (U256::ZERO, U256::from(10)))]))
            .build(),
        vec![vec![Some(Receipt {
            tx_type: TxType::Eip2930,
            success: true,
            cumulative_gas_used: 300,
            logs: vec![Log::new_unchecked(
                Address::new([0x60; 20]),
                vec![B256::with_last_byte(1), B256::with_last_byte(2)],
                Bytes::default(),
            )],
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        })]]
        .into(),
        number,
        Vec::new(),
    );

    let state_root = bundle_state_root(&execution_outcome);
    assert_eq!(
        state_root,
        b256!("5d035ccb3e75a9057452ff060b773b213ec1fc353426174068edfc3971a0b6bd")
    );

    let mut block = SealedBlock::decode(&mut BLOCK_RLP.as_slice()).unwrap();
    block.withdrawals = Some(Withdrawals::new(vec![Withdrawal::default()]));
    let mut header = block.header.clone().unseal();
    header.number = number;
    header.state_root = state_root;
    header.parent_hash = B256::ZERO;
    block.header = header.seal_slow();

    (SealedBlockWithSenders { block, senders: vec![Address::new([0x30; 20])] }, execution_outcome)
}

/// Block two that points to block 1
fn block2(
    number: BlockNumber,
    parent_hash: B256,
    prev_execution_outcome: &ExecutionOutcome,
) -> (SealedBlockWithSenders, ExecutionOutcome) {
    // block changes
    let account: Address = [0x60; 20].into();
    let slot = U256::from(5);

    let execution_outcome = ExecutionOutcome::new(
        BundleState::builder(number..=number)
            .state_present_account_info(
                account,
                AccountInfo { nonce: 3, balance: U256::from(20), ..Default::default() },
            )
            .state_storage(account, HashMap::from([(slot, (U256::ZERO, U256::from(15)))]))
            .revert_account_info(
                number,
                account,
                Some(Some(AccountInfo { nonce: 1, balance: U256::from(10), ..Default::default() })),
            )
            .revert_storage(number, account, Vec::from([(slot, U256::from(10))]))
            .build(),
        vec![vec![Some(Receipt {
            tx_type: TxType::Eip1559,
            success: false,
            cumulative_gas_used: 400,
            logs: vec![Log::new_unchecked(
                Address::new([0x61; 20]),
                vec![B256::with_last_byte(3), B256::with_last_byte(4)],
                Bytes::default(),
            )],
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        })]]
        .into(),
        number,
        Vec::new(),
    );

    let mut extended = prev_execution_outcome.clone();
    extended.extend(execution_outcome.clone());
    let state_root = bundle_state_root(&extended);
    assert_eq!(
        state_root,
        b256!("90101a13dd059fa5cca99ed93d1dc23657f63626c5b8f993a2ccbdf7446b64f8")
    );

    let mut block = SealedBlock::decode(&mut BLOCK_RLP.as_slice()).unwrap();

    block.withdrawals = Some(Withdrawals::new(vec![Withdrawal::default()]));
    let mut header = block.header.clone().unseal();
    header.number = number;
    header.state_root = state_root;
    // parent_hash points to block1 hash
    header.parent_hash = parent_hash;
    block.header = header.seal_slow();

    (SealedBlockWithSenders { block, senders: vec![Address::new([0x31; 20])] }, execution_outcome)
}

/// Block three that points to block 2
fn block3(
    number: BlockNumber,
    parent_hash: B256,
    prev_execution_outcome: &ExecutionOutcome,
) -> (SealedBlockWithSenders, ExecutionOutcome) {
    let address_range = 1..=20;
    let slot_range = 1..=100;

    let mut bundle_state_builder = BundleState::builder(number..=number);
    for idx in address_range {
        let address = Address::with_last_byte(idx);
        bundle_state_builder = bundle_state_builder
            .state_present_account_info(
                address,
                AccountInfo { nonce: 1, balance: U256::from(idx), ..Default::default() },
            )
            .state_storage(
                address,
                HashMap::from_iter(
                    slot_range
                        .clone()
                        .map(|slot| (U256::from(slot), (U256::ZERO, U256::from(slot)))),
                ),
            )
            .revert_account_info(number, address, Some(None))
            .revert_storage(number, address, Vec::new());
    }
    let execution_outcome = ExecutionOutcome::new(
        bundle_state_builder.build(),
        vec![vec![Some(Receipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 400,
            logs: vec![Log::new_unchecked(
                Address::new([0x61; 20]),
                vec![B256::with_last_byte(3), B256::with_last_byte(4)],
                Bytes::default(),
            )],
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        })]]
        .into(),
        number,
        Vec::new(),
    );

    let mut extended = prev_execution_outcome.clone();
    extended.extend(execution_outcome.clone());
    let state_root = bundle_state_root(&extended);

    let mut block = SealedBlock::decode(&mut BLOCK_RLP.as_slice()).unwrap();
    block.withdrawals = Some(Withdrawals::new(vec![Withdrawal::default()]));
    let mut header = block.header.clone().unseal();
    header.number = number;
    header.state_root = state_root;
    // parent_hash points to block1 hash
    header.parent_hash = parent_hash;
    block.header = header.seal_slow();

    (SealedBlockWithSenders { block, senders: vec![Address::new([0x31; 20])] }, execution_outcome)
}

/// Block four that points to block 3
fn block4(
    number: BlockNumber,
    parent_hash: B256,
    prev_execution_outcome: &ExecutionOutcome,
) -> (SealedBlockWithSenders, ExecutionOutcome) {
    let address_range = 1..=20;
    let slot_range = 1..=100;

    let mut bundle_state_builder = BundleState::builder(number..=number);
    for idx in address_range {
        let address = Address::with_last_byte(idx);
        // increase balance for every even account and destroy every odd
        bundle_state_builder = if idx % 2 == 0 {
            bundle_state_builder
                .state_present_account_info(
                    address,
                    AccountInfo { nonce: 1, balance: U256::from(idx * 2), ..Default::default() },
                )
                .state_storage(
                    address,
                    HashMap::from_iter(
                        slot_range.clone().map(|slot| {
                            (U256::from(slot), (U256::from(slot), U256::from(slot * 2)))
                        }),
                    ),
                )
        } else {
            bundle_state_builder.state_address(address).state_storage(
                address,
                HashMap::from_iter(
                    slot_range
                        .clone()
                        .map(|slot| (U256::from(slot), (U256::from(slot), U256::ZERO))),
                ),
            )
        };
        // record previous account info
        bundle_state_builder = bundle_state_builder
            .revert_account_info(
                number,
                address,
                Some(Some(AccountInfo {
                    nonce: 1,
                    balance: U256::from(idx),
                    ..Default::default()
                })),
            )
            .revert_storage(
                number,
                address,
                Vec::from_iter(slot_range.clone().map(|slot| (U256::from(slot), U256::from(slot)))),
            );
    }
    let execution_outcome = ExecutionOutcome::new(
        bundle_state_builder.build(),
        vec![vec![Some(Receipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 400,
            logs: vec![Log::new_unchecked(
                Address::new([0x61; 20]),
                vec![B256::with_last_byte(3), B256::with_last_byte(4)],
                Bytes::default(),
            )],
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        })]]
        .into(),
        number,
        Vec::new(),
    );

    let mut extended = prev_execution_outcome.clone();
    extended.extend(execution_outcome.clone());
    let state_root = bundle_state_root(&extended);

    let mut block = SealedBlock::decode(&mut BLOCK_RLP.as_slice()).unwrap();
    block.withdrawals = Some(Withdrawals::new(vec![Withdrawal::default()]));
    let mut header = block.header.clone().unseal();
    header.number = number;
    header.state_root = state_root;
    // parent_hash points to block1 hash
    header.parent_hash = parent_hash;
    block.header = header.seal_slow();

    (SealedBlockWithSenders { block, senders: vec![Address::new([0x31; 20])] }, execution_outcome)
}

/// Block five that points to block 4
fn block5(
    number: BlockNumber,
    parent_hash: B256,
    prev_execution_outcome: &ExecutionOutcome,
) -> (SealedBlockWithSenders, ExecutionOutcome) {
    let address_range = 1..=20;
    let slot_range = 1..=100;

    let mut bundle_state_builder = BundleState::builder(number..=number);
    for idx in address_range {
        let address = Address::with_last_byte(idx);
        // update every even account and recreate every odd only with half of slots
        bundle_state_builder = bundle_state_builder
            .state_present_account_info(
                address,
                AccountInfo { nonce: 1, balance: U256::from(idx * 2), ..Default::default() },
            )
            .state_storage(
                address,
                HashMap::from_iter(
                    slot_range
                        .clone()
                        .take(50)
                        .map(|slot| (U256::from(slot), (U256::from(slot), U256::from(slot * 4)))),
                ),
            );
        bundle_state_builder = if idx % 2 == 0 {
            bundle_state_builder
                .revert_account_info(
                    number,
                    address,
                    Some(Some(AccountInfo {
                        nonce: 1,
                        balance: U256::from(idx * 2),
                        ..Default::default()
                    })),
                )
                .revert_storage(
                    number,
                    address,
                    Vec::from_iter(
                        slot_range.clone().map(|slot| (U256::from(slot), U256::from(slot * 2))),
                    ),
                )
        } else {
            bundle_state_builder.revert_address(number, address)
        };
    }
    let execution_outcome = ExecutionOutcome::new(
        bundle_state_builder.build(),
        vec![vec![Some(Receipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 400,
            logs: vec![Log::new_unchecked(
                Address::new([0x61; 20]),
                vec![B256::with_last_byte(3), B256::with_last_byte(4)],
                Bytes::default(),
            )],
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        })]]
        .into(),
        number,
        Vec::new(),
    );

    let mut extended = prev_execution_outcome.clone();
    extended.extend(execution_outcome.clone());
    let state_root = bundle_state_root(&extended);

    let mut block = SealedBlock::decode(&mut BLOCK_RLP.as_slice()).unwrap();
    block.withdrawals = Some(Withdrawals::new(vec![Withdrawal::default()]));
    let mut header = block.header.clone().unseal();
    header.number = number;
    header.state_root = state_root;
    // parent_hash points to block1 hash
    header.parent_hash = parent_hash;
    block.header = header.seal_slow();

    (SealedBlockWithSenders { block, senders: vec![Address::new([0x31; 20])] }, execution_outcome)
}
