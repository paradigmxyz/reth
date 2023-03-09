use std::{collections::BTreeMap, ops::DerefMut};

use crate::{
    execution_result::{
        AccountChangeSet, AccountInfoChangeSet, ExecutionResult, TransactionChangeSet,
    },
    insert_canonical_block, Transaction,
};
use reth_db::{
    database::Database, mdbx::test_utils::create_test_rw_db, models::StoredBlockBody, tables,
    transaction::DbTxMut,
};
use reth_primitives::{
    hex_literal::hex, proofs::EMPTY_ROOT, Account, ChainSpecBuilder, Header, Receipt, SealedBlock,
    SealedBlockWithSenders, Withdrawal, H160, H256, MAINNET, U256,
};
use reth_rlp::Decodable;

pub fn assert_genesis_block<DB: Database>(tx: &Transaction<'_, DB>, g: SealedBlock) {
    let n = g.number;
    let h = H256::zero();
    // check if all tables are empty
    assert_eq!(tx.table::<tables::Headers>().unwrap(), vec![(g.number, g.header.clone().unseal())]);

    assert_eq!(tx.table::<tables::HeaderNumbers>().unwrap(), vec![(h, n)]);
    assert_eq!(tx.table::<tables::CanonicalHeaders>().unwrap(), vec![(n, h)]);
    assert_eq!(tx.table::<tables::HeaderTD>().unwrap(), vec![(n, g.difficulty.into())]);
    assert_eq!(tx.table::<tables::BlockBodies>().unwrap(), vec![(0, StoredBlockBody::default())]);
    assert_eq!(tx.table::<tables::BlockOmmers>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::BlockWithdrawals>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::Transactions>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::TxHashNumber>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::Receipts>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::PlainAccountState>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::PlainStorageState>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::AccountHistory>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::StorageHistory>().unwrap(), vec![]);
    // Bytecodes are not reverted assert_eq!(tx.table::<tables::Bytecodes>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::BlockTransitionIndex>().unwrap(), vec![(n, 0)]);
    assert_eq!(tx.table::<tables::TxTransitionIndex>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::AccountChangeSet>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::StorageChangeSet>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::HashedAccount>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::HashedStorage>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::AccountsTrie>().unwrap(), vec![(EMPTY_ROOT, vec![0x80])]);
    assert_eq!(tx.table::<tables::StoragesTrie>().unwrap(), vec![]);
    assert_eq!(tx.table::<tables::TxSenders>().unwrap(), vec![]);
    // SyncStage is not updated in tests
}

pub fn genesis() -> SealedBlock {
    SealedBlock {
        header: Header { number: 0, difficulty: U256::from(1), ..Default::default() }
            .seal(H256::zero()),
        body: vec![],
        ommers: vec![],
        withdrawals: Some(vec![]),
    }
}

pub fn block1() -> (SealedBlockWithSenders, ExecutionResult) {
    let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
    let mut block = SealedBlock::decode(&mut block_rlp).unwrap();
    block.withdrawals = Some(vec![Withdrawal::default()]);
    let mut header = block.header.clone().unseal();
    header.number = 1;
    header.state_root =
        H256(hex!("f7c6a43dd9551fb93b1b72e47c1a16d6f3ea9e87d6088aa56514ffddf53c65d3"));
    header.parent_hash = H256::zero();
    block.header = header.seal_slow();

    let mut account_changeset = AccountChangeSet::default();
    // storage will be moved
    account_changeset.account = AccountInfoChangeSet::Created {
        new: Account { nonce: 1, balance: U256::from(10), bytecode_hash: None },
    };
    account_changeset.storage.insert(U256::from(5), (U256::ZERO, U256::from(10)));

    let exec_res = ExecutionResult {
        tx_changesets: vec![TransactionChangeSet {
            receipt: Receipt::default(), /* receipts are not saved. */
            changeset: BTreeMap::from([(H160([50; 20]), account_changeset.clone())]),
            new_bytecodes: BTreeMap::from([]),
        }],
        block_changesets: BTreeMap::from([(H160([51; 20]), account_changeset.account)]),
    };

    (SealedBlockWithSenders { block, senders: vec![H160([3; 20])] }, exec_res)
}

pub fn block2() -> (SealedBlockWithSenders, ExecutionResult) {
    let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
    let mut block = SealedBlock::decode(&mut block_rlp).unwrap();
    block.withdrawals = Some(vec![Withdrawal::default()]);
    let mut header = block.header.clone().unseal();
    header.number = 2;
    header.state_root =
        H256(hex!("8d1570fc7c7e25de7eab9ae2611ef4752f89b8d1e076916389180a29306dd509"));
    // parent_hash points to block1 hash
    header.parent_hash =
        H256(hex!("19a369c389eff93f96511da6a75917ae51ef3d11172a200d5315820bf2a4460e"));
    block.header = header.seal_slow();

    let mut account_changeset = AccountChangeSet::default();
    // storage will be moved
    let info_changeset = AccountInfoChangeSet::Changed {
        old: Account { nonce: 1, balance: U256::from(10), bytecode_hash: None },
        new: Account { nonce: 2, balance: U256::from(15), bytecode_hash: None },
    };
    account_changeset.account = info_changeset.clone();
    account_changeset.storage.insert(U256::from(5), (U256::from(10), U256::from(15)));

    let block_changeset = AccountInfoChangeSet::Changed {
        old: Account { nonce: 2, balance: U256::from(15), bytecode_hash: None },
        new: Account { nonce: 3, balance: U256::from(20), bytecode_hash: None },
    };
    let exec_res = ExecutionResult {
        tx_changesets: vec![TransactionChangeSet {
            receipt: Receipt::default(), /* receipts are not saved. */
            changeset: BTreeMap::from([(H160([50; 20]), account_changeset.clone())]),
            new_bytecodes: BTreeMap::from([]),
        }],
        block_changesets: BTreeMap::from([(H160([50; 20]), block_changeset)]),
    };

    (SealedBlockWithSenders { block, senders: vec![H160([3; 20])] }, exec_res)
}
