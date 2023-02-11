use crate::{
    config::{revm_spec, WEI_2ETH, WEI_3ETH, WEI_5ETH},
    execution_result::{
        AccountChangeSet, AccountInfoChangeSet, ExecutionResult, TransactionChangeSet,
    },
    revm_wrap::{self, into_reth_log, to_reth_acc, SubState},
};
use hashbrown::hash_map::Entry;
use reth_interfaces::executor::{BlockExecutor, Error};
use reth_primitives::{
    bloom::logs_bloom, Account, Address, Block, Bloom, ChainSpec, Hardfork, Head, Header, Log,
    Receipt, TransactionSigned, H160, H256, U256,
};
use reth_provider::StateProvider;
use revm::{
    db::AccountState,
    primitives::{
        Account as RevmAccount, AccountInfo, AnalysisKind, Bytecode, ResultAndState, SpecId,
    },
    EVM,
};
use std::collections::BTreeMap;

/// Main block executor
pub struct Executor<'a, DB>
where
    DB: StateProvider,
{
    chain_spec: &'a ChainSpec,
    evm: EVM<&'a mut SubState<DB>>,
    /// Enable revm inspector printer.
    /// In execution this will print opcode level traces directly to console.
    pub use_printer_tracer: bool,
}

impl<'a, DB> Executor<'a, DB>
where
    DB: StateProvider,
{
    fn new(chain_spec: &'a ChainSpec, db: &'a mut SubState<DB>) -> Self {
        let mut evm = EVM::new();
        evm.database(db);
        Executor { chain_spec, evm, use_printer_tracer: false }
    }

    fn db(&mut self) -> &mut SubState<DB> {
        self.evm.db().expect("db to not be moved")
    }

    fn recover_senders(
        &self,
        body: &[TransactionSigned],
        senders: Option<Vec<Address>>,
    ) -> Result<Vec<Address>, Error> {
        if let Some(senders) = senders {
            if body.len() == senders.len() {
                Ok(senders)
            } else {
                Err(Error::SenderRecoveryError)
            }
        } else {
            body.iter().map(|tx| tx.recover_signer().ok_or(Error::SenderRecoveryError)).collect()
        }
    }

    fn init_block_env(&mut self, header: &Header, total_difficulty: U256) {
        let spec_id = revm_spec(
            self.chain_spec,
            Head {
                number: header.number,
                timestamp: header.timestamp,
                total_difficulty,
                ..Default::default()
            },
        );

        self.evm.env.cfg.chain_id = U256::from(self.chain_spec.chain().id());
        self.evm.env.cfg.spec_id = spec_id;
        self.evm.env.cfg.perf_all_precompiles_have_balance = false;
        self.evm.env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

        revm_wrap::fill_block_env(&mut self.evm.env.block, header, spec_id >= SpecId::MERGE);
    }

    /// Commit change to database and return change diff that is used to update state and create
    /// history index
    ///
    /// ChangeDiff consists of:
    ///     address->AccountChangeSet (It contains old and new account info,storage wipe flag, and
    /// old/new storage)     bytecode_hash->bytecodes mapping
    ///
    /// BTreeMap is used to have sorted values
    fn commit_changes(
        &mut self,
        changes: hashbrown::HashMap<H160, RevmAccount>,
    ) -> (BTreeMap<Address, AccountChangeSet>, BTreeMap<H256, Bytecode>) {
        let db = self.db();

        let mut change = BTreeMap::new();
        let mut new_bytecodes = BTreeMap::new();
        // iterate over all changed accounts
        for (address, account) in changes {
            if account.is_destroyed {
                // get old account that we are destroying.
                let db_account = match db.accounts.entry(address) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(_entry) => {
                        panic!("Left panic to critically jumpout if happens, as every account should be hot loaded.");
                    }
                };
                // Insert into `change` a old account and None for new account
                // and mark storage to be mapped
                change.insert(
                    address,
                    AccountChangeSet {
                        account: AccountInfoChangeSet::Destroyed {
                            old: to_reth_acc(&db_account.info),
                        },
                        storage: BTreeMap::new(),
                        wipe_storage: true,
                    },
                );
                // clear cached DB and mark account as not existing
                db_account.storage.clear();
                db_account.account_state = AccountState::NotExisting;
                db_account.info = AccountInfo::default();

                continue
            } else {
                // check if account code is new or old.
                // does it exist inside cached contracts if it doesn't it is new bytecode that
                // we are inserting inside `change`
                if let Some(ref code) = account.info.code {
                    if !code.is_empty() {
                        match db.contracts.entry(account.info.code_hash) {
                            Entry::Vacant(entry) => {
                                entry.insert(code.clone());
                                new_bytecodes.insert(H256(account.info.code_hash.0), code.clone());
                            }
                            Entry::Occupied(mut entry) => {
                                entry.insert(code.clone());
                            }
                        }
                    }
                }

                // get old account that is going to be overwritten or none if it does not exist
                // and get new account that was just inserted. new account mut ref is used for
                // inserting storage
                let (account_info_changeset, new_account) = match db.accounts.entry(address) {
                    Entry::Vacant(entry) => {
                        let entry = entry.insert(Default::default());
                        entry.info = account.info.clone();
                        // account was not existing, so this means new account is created
                        (AccountInfoChangeSet::Created { new: to_reth_acc(&entry.info) }, entry)
                    }
                    Entry::Occupied(entry) => {
                        let entry = entry.into_mut();

                        // account is present inside cache but is marked as NotExisting.
                        let account_changeset =
                            if matches!(entry.account_state, AccountState::NotExisting) {
                                AccountInfoChangeSet::Created { new: to_reth_acc(&account.info) }
                            } else if entry.info != account.info {
                                AccountInfoChangeSet::Changed {
                                    old: to_reth_acc(&entry.info),
                                    new: to_reth_acc(&account.info),
                                }
                            } else {
                                AccountInfoChangeSet::NoChange
                            };
                        entry.info = account.info.clone();
                        (account_changeset, entry)
                    }
                };

                let mut wipe_storage = false;

                new_account.account_state = if account.storage_cleared {
                    new_account.storage.clear();
                    wipe_storage = true;
                    AccountState::StorageCleared
                } else {
                    AccountState::Touched
                };

                // Insert storage.
                let mut storage = BTreeMap::new();

                // insert storage into new db account.
                new_account.storage.extend(account.storage.into_iter().map(|(key, value)| {
                    storage.insert(key, (value.original_value(), value.present_value()));
                    (key, value.present_value())
                }));

                // Insert into change.
                change.insert(
                    address,
                    AccountChangeSet { account: account_info_changeset, storage, wipe_storage },
                );
            }
        }
        (change, new_bytecodes)
    }

    /// Calculate Block reward changeset
    pub fn block_reward_changeset(
        &mut self,
        header: &Header,
        total_difficulty: U256,
        ommers: &[Header],
    ) -> Result<Option<BTreeMap<H160, AccountInfoChangeSet>>, Error> {
        // NOTE: Related to Ethereum reward change, for other network this is probably going to be
        // moved to config.

        // From yellowpapper Page 15:
        // 11.3. Reward Application. The application of rewards to a block involves raising the
        // balance of the accounts of the beneficiary address of the block and each ommer by
        // a certain amount. We raise the block’s beneficiary account by Rblock; for each
        // ommer, we raise the block’s beneficiary by an additional 1/32 of the block reward
        // and the beneficiary of the ommer gets rewarded depending on the blocknumber.
        // Formally we define the function Ω:
        if self.chain_spec.fork(Hardfork::Paris).active_at_ttd(total_difficulty) {
            None
        } else if self.chain_spec.fork(Hardfork::Petersburg).active_at_block(header.number) {
            Some(WEI_2ETH)
        } else if self.chain_spec.fork(Hardfork::Byzantium).active_at_block(header.number) {
            Some(WEI_3ETH)
        } else {
            Some(WEI_5ETH)
        }
        .map(|reward| -> Result<_, _> {
            let mut reward_beneficiaries: BTreeMap<H160, u128> = BTreeMap::new();
            // Calculate Uncle reward
            // OpenEthereum code: https://github.com/openethereum/openethereum/blob/6c2d392d867b058ff867c4373e40850ca3f96969/crates/ethcore/src/ethereum/ethash.rs#L319-L333
            for ommer in ommers {
                let ommer_reward = ((8 + ommer.number - header.number) as u128 * reward) >> 3;
                // From yellowpaper Page 15:
                // If there are collisions of the beneficiary addresses between ommers and the block
                // (i.e. two ommers with the same beneficiary address or an ommer with the
                // same beneficiary address as the present block), additions are applied
                // cumulatively
                *reward_beneficiaries.entry(ommer.beneficiary).or_default() += ommer_reward;
            }
            // insert main block reward
            *reward_beneficiaries.entry(header.beneficiary).or_default() +=
                reward + (reward >> 5) * ommers.len() as u128;

            //
            let db = self.db();

            // create changesets for beneficiaries rewards (Main block and ommers);
            reward_beneficiaries
                .into_iter()
                .map(|(beneficiary, reward)| -> Result<_, _> {
                    let changeset = db
                        .load_account(beneficiary)
                        .map_err(|_| Error::ProviderError)
                        // if account is present append `Changed` changeset for block reward
                        .map(|db_acc| {
                            let old = to_reth_acc(&db_acc.info);
                            let mut new = old;
                            new.balance += U256::from(reward);
                            db_acc.info.balance = new.balance;
                            match db_acc.account_state {
                                AccountState::NotExisting => {
                                    // if account was not existing that means that storage is not
                                    // present.
                                    db_acc.account_state = AccountState::StorageCleared;

                                    // if account was not present append `Created` changeset
                                    AccountInfoChangeSet::Created {
                                        new: Account {
                                            nonce: 0,
                                            balance: new.balance,
                                            bytecode_hash: None,
                                        },
                                    }
                                }

                                AccountState::StorageCleared |
                                AccountState::Touched |
                                AccountState::None => {
                                    // If account is None that means that EVM didn't touch it.
                                    // we are changing the state to Touched as account can have
                                    // storage in db.
                                    if db_acc.account_state == AccountState::None {
                                        db_acc.account_state = AccountState::Touched;
                                    }
                                    // if account was present, append changed changeset.
                                    AccountInfoChangeSet::Changed { new, old }
                                }
                            }
                        })?;
                    Ok((beneficiary, changeset))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        })
        .transpose()
    }

    /// Irregular state change at Ethereum DAO hardfork
    pub fn dao_fork_changeset(&mut self) -> Result<BTreeMap<H160, AccountInfoChangeSet>, Error> {
        let db = self.db();

        let mut drained_balance = U256::ZERO;

        // drain all accounts ether
        let mut changesets = crate::eth_dao_fork::DAO_HARDKFORK_ACCOUNTS
            .iter()
            .map(|&address| {
                let db_account = db.load_account(address).map_err(|_| Error::ProviderError)?;
                let old = to_reth_acc(&db_account.info);
                // drain balance
                drained_balance += core::mem::take(&mut db_account.info.balance);
                let new = to_reth_acc(&db_account.info);
                // assume it is changeset as it is irregular state change
                Ok((address, AccountInfoChangeSet::Changed { new, old }))
            })
            .collect::<Result<BTreeMap<H160, AccountInfoChangeSet>, _>>()?;

        // add drained ether to beneficiary.
        let beneficiary = crate::eth_dao_fork::DAO_HARDFORK_BENEFICIARY;

        let beneficiary_db_account =
            db.load_account(beneficiary).map_err(|_| Error::ProviderError)?;
        let old = to_reth_acc(&beneficiary_db_account.info);
        beneficiary_db_account.info.balance += drained_balance;
        let new = to_reth_acc(&beneficiary_db_account.info);

        let beneficiary_changeset = AccountInfoChangeSet::Changed { new, old };

        // insert changeset
        changesets.insert(beneficiary, beneficiary_changeset);

        Ok(changesets)
    }
}

impl<'a, DB> BlockExecutor<ExecutionResult> for Executor<'a, DB>
where
    DB: StateProvider,
{
    fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<ExecutionResult, Error> {
        let Block { header, body, ommers } = block;
        let senders = self.recover_senders(body, senders)?;

        self.init_block_env(header, total_difficulty);

        let mut cumulative_gas_used = 0;
        // output of execution
        let mut changesets = Vec::with_capacity(body.len());

        for (transaction, sender) in body.iter().zip(senders.into_iter()) {
            // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(Error::TransactionGasLimitMoreThenAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                })
            }

            // Fill revm structure.
            revm_wrap::fill_tx_env(&mut self.evm.env.tx, transaction, sender);

            // Execute transaction.
            let out = if self.use_printer_tracer {
                // execution with inspector.
                let out = self.evm.inspect(revm::inspectors::CustomPrintTracer::default());

                tracing::trace!(target:"evm","Executed transaction: {:?},
                    \nExecution output:{out:?}:
                    \nTransaction data:{transaction:?}
                    \nenv data:{:?}",transaction.hash(),self.evm.env);
                out
            } else {
                // main execution.
                self.evm.transact()
            };

            // cast the error and extract returnables.
            let ResultAndState { result, state } = out.map_err(|e| Error::EVM(format!("{e:?}")))?;

            // commit changes
            let (changeset, new_bytecodes) = self.commit_changes(state);

            // append gas used
            cumulative_gas_used += result.gas_used();

            // cast revm logs to reth logs
            let logs: Vec<Log> = result.logs().into_iter().map(into_reth_log).collect();

            // Push transaction changeset and calculate header bloom filter for receipt.
            changesets.push(TransactionChangeSet {
                receipt: Receipt {
                    tx_type: transaction.tx_type(),
                    // Success flag was added in `EIP-658: Embedding transaction status code in
                    // receipts`.
                    success: result.is_success(),
                    cumulative_gas_used,
                    bloom: logs_bloom(logs.iter()),
                    logs,
                },
                changeset,
                new_bytecodes,
            });
        }

        // Check if gas used matches the value set in header.
        if header.gas_used != cumulative_gas_used {
            return Err(Error::BlockGasUsed { got: cumulative_gas_used, expected: header.gas_used })
        }

        let mut block_reward = self.block_reward_changeset(header, total_difficulty, ommers)?;

        if self.chain_spec.fork(Hardfork::Dao).transitions_at_block(header.number) {
            let mut irregular_state_changeset = self.dao_fork_changeset()?;
            irregular_state_changeset.extend(block_reward.take().unwrap_or_default().into_iter());
            block_reward = Some(irregular_state_changeset);
        }

        Ok(ExecutionResult { changesets, block_reward })
    }
}

/// Execute and verify block
pub fn execute_and_verify_receipt<DB: StateProvider>(
    block: &Block,
    total_difficulty: U256,
    senders: Option<Vec<Address>>,
    chain_spec: &ChainSpec,
    db: &mut SubState<DB>,
) -> Result<ExecutionResult, Error> {
    let execution_result = execute(block, total_difficulty, senders, chain_spec, db)?;

    let receipts_iter = execution_result.changesets.iter().map(|changeset| &changeset.receipt);

    if chain_spec.fork(Hardfork::Byzantium).active_at_block(block.header.number) {
        verify_receipt(block.header.receipts_root, block.header.logs_bloom, receipts_iter)?;
    }

    // TODO Before Byzantium, receipts contained state root that would mean that expensive operation
    // as hashing that is needed for state root got calculated in every transaction
    // This was replaced with is_success flag.
    // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658

    Ok(execution_result)
}

/// Verify receipts
pub fn verify_receipt<'a>(
    expected_receipts_root: H256,
    expected_logs_bloom: Bloom,
    receipts: impl Iterator<Item = &'a Receipt> + Clone,
) -> Result<(), Error> {
    // Check receipts root.
    let receipts_root = reth_primitives::proofs::calculate_receipt_root(receipts.clone());
    if receipts_root != expected_receipts_root {
        return Err(Error::ReceiptRootDiff { got: receipts_root, expected: expected_receipts_root })
    }

    // Create header log bloom.
    let logs_bloom = receipts.fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
    if logs_bloom != expected_logs_bloom {
        return Err(Error::BloomLogDiff {
            expected: Box::new(expected_logs_bloom),
            got: Box::new(logs_bloom),
        })
    }
    Ok(())
}

/// Verify block. Execute all transaction and compare results.
/// Returns ChangeSet on transaction granularity.
/// NOTE: If block reward is still active (Before Paris/Merge) we would return
/// additional TransactionStatechangeset for account that receives the reward.
pub fn execute<DB: StateProvider>(
    block: &Block,
    total_difficulty: U256,
    senders: Option<Vec<Address>>,
    chain_spec: &ChainSpec,
    db: &mut SubState<DB>,
) -> Result<ExecutionResult, Error> {
    let mut executor = Executor::new(chain_spec, db);
    executor.execute(block, total_difficulty, senders)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::revm_wrap::State;
    use reth_primitives::{
        hex_literal::hex, keccak256, Account, Address, Bytes, ChainSpecBuilder, ForkCondition,
        SealedBlock, StorageKey, H160, H256, MAINNET, U256,
    };
    use reth_provider::{AccountProvider, BlockHashProvider, StateProvider};
    use reth_rlp::Decodable;

    use super::*;

    #[derive(Debug, Default, Clone, Eq, PartialEq)]
    struct StateProviderTest {
        accounts: HashMap<Address, (HashMap<StorageKey, U256>, Account)>,
        contracts: HashMap<H256, Bytes>,
        block_hash: HashMap<U256, H256>,
    }

    impl StateProviderTest {
        /// Insert account.
        fn insert_account(
            &mut self,
            address: Address,
            mut account: Account,
            bytecode: Option<Bytes>,
            storage: HashMap<StorageKey, U256>,
        ) {
            if let Some(bytecode) = bytecode {
                let hash = keccak256(&bytecode);
                account.bytecode_hash = Some(hash);
                self.contracts.insert(hash, bytecode);
            }
            self.accounts.insert(address, (storage, account));
        }
    }

    impl AccountProvider for StateProviderTest {
        fn basic_account(&self, address: Address) -> reth_interfaces::Result<Option<Account>> {
            let ret = Ok(self.accounts.get(&address).map(|(_, acc)| *acc));
            ret
        }
    }

    impl BlockHashProvider for StateProviderTest {
        fn block_hash(&self, number: U256) -> reth_interfaces::Result<Option<H256>> {
            Ok(self.block_hash.get(&number).cloned())
        }
    }

    impl StateProvider for StateProviderTest {
        fn storage(
            &self,
            account: Address,
            storage_key: reth_primitives::StorageKey,
        ) -> reth_interfaces::Result<Option<reth_primitives::StorageValue>> {
            Ok(self
                .accounts
                .get(&account)
                .and_then(|(storage, _)| storage.get(&storage_key).cloned()))
        }

        fn bytecode_by_hash(&self, code_hash: H256) -> reth_interfaces::Result<Option<Bytes>> {
            Ok(self.contracts.get(&code_hash).cloned())
        }
    }

    #[test]
    fn sanity_execution() {
        // Got rlp block from: src/GeneralStateTestsFiller/stChainId/chainIdGasCostFiller.json

        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();

        let mut ommer = Header::default();
        let ommer_beneficiary = H160(hex!("3000000000000000000000000000000000000000"));
        ommer.beneficiary = ommer_beneficiary;
        ommer.number = block.number;
        let ommers = vec![ommer];

        let block = Block { header: block.header.unseal(), body: block.body, ommers };

        let mut db = StateProviderTest::default();

        let account1 = H160(hex!("1000000000000000000000000000000000000000"));
        let account2 = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        let account3 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));

        // pre staet
        db.insert_account(
            account1,
            Account { balance: U256::ZERO, nonce: 0x00, bytecode_hash: None },
            Some(hex!("5a465a905090036002900360015500").into()),
            HashMap::new(),
        );

        let account3_old_info = Account {
            balance: U256::from(0x3635c9adc5dea00000u128),
            nonce: 0x00,
            bytecode_hash: None,
        };

        db.insert_account(
            account3,
            Account {
                balance: U256::from(0x3635c9adc5dea00000u128),
                nonce: 0x00,
                bytecode_hash: None,
            },
            None,
            HashMap::new(),
        );

        // spec at berlin fork
        let chain_spec = ChainSpecBuilder::mainnet().berlin_activated().build();

        let mut db = SubState::new(State::new(db));

        // execute chain and verify receipts
        let out =
            execute_and_verify_receipt(&block, U256::ZERO, None, &chain_spec, &mut db).unwrap();

        assert_eq!(out.changesets.len(), 1, "Should executed one transaction");

        let changesets = out.changesets[0].clone();
        assert_eq!(changesets.new_bytecodes.len(), 0, "Should have zero new bytecodes");

        let account1_info = Account { balance: U256::ZERO, nonce: 0x00, bytecode_hash: None };
        let account2_info = Account {
            balance: U256::from(0x1bc16d674ece94bau128 - 0x1bc16d674ec80000u128), /* decrease for
                                                                                   * block reward */
            nonce: 0x00,
            bytecode_hash: None,
        };
        let account3_info = Account {
            balance: U256::from(0x3635c9adc5de996b46u128),
            nonce: 0x01,
            bytecode_hash: None,
        };

        let block_reward = U256::from(WEI_2ETH + (WEI_2ETH >> 5));

        // Check if cache is set
        // account1
        let cached_acc1 = db.accounts.get(&account1).unwrap();
        assert_eq!(cached_acc1.info.balance, account1_info.balance);
        assert_eq!(cached_acc1.info.nonce, account1_info.nonce);
        assert!(matches!(cached_acc1.account_state, AccountState::Touched));
        assert_eq!(cached_acc1.storage.len(), 1);
        assert_eq!(cached_acc1.storage.get(&U256::from(1)), Some(&U256::from(2)));

        // account2 Block reward
        let cached_acc2 = db.accounts.get(&account2).unwrap();
        assert_eq!(cached_acc2.info.balance, account2_info.balance + block_reward);
        assert_eq!(cached_acc2.info.nonce, account2_info.nonce);
        assert_eq!(cached_acc2.account_state, AccountState::Touched);
        assert_eq!(cached_acc2.storage.len(), 0);

        // account3
        let cached_acc3 = db.accounts.get(&account3).unwrap();
        assert_eq!(cached_acc3.info.balance, account3_info.balance);
        assert_eq!(cached_acc3.info.nonce, account3_info.nonce);
        assert!(matches!(cached_acc3.account_state, AccountState::Touched));
        assert_eq!(cached_acc3.storage.len(), 0);

        assert_eq!(
            changesets.changeset.get(&account1).unwrap().account,
            AccountInfoChangeSet::NoChange,
            "No change to account"
        );
        assert_eq!(
            changesets.changeset.get(&account2).unwrap().account,
            AccountInfoChangeSet::Created { new: account2_info },
            "New account"
        );
        assert_eq!(
            changesets.changeset.get(&account3).unwrap().account,
            AccountInfoChangeSet::Changed { old: account3_old_info, new: account3_info },
            "Change to account state"
        );

        // check block rewards changeset.
        let mut block_rewarded_acc_info = account2_info;
        // add Blocks 2 eth reward and 2>>5 for one ommer
        block_rewarded_acc_info.balance += block_reward;

        // check block reward changeset
        assert_eq!(
            out.block_reward,
            Some(BTreeMap::from([
                (
                    account2,
                    AccountInfoChangeSet::Changed {
                        new: block_rewarded_acc_info,
                        old: account2_info
                    }
                ),
                (
                    ommer_beneficiary,
                    AccountInfoChangeSet::Created {
                        new: Account {
                            nonce: 0,
                            balance: U256::from((8 * WEI_2ETH) >> 3),
                            bytecode_hash: None
                        }
                    }
                )
            ]))
        );

        assert_eq!(changesets.new_bytecodes.len(), 0, "No new bytecodes");

        // check storage
        let storage = &changesets.changeset.get(&account1).unwrap().storage;
        assert_eq!(storage.len(), 1, "Only one storage change");
        assert_eq!(
            storage.get(&U256::from(1)),
            Some(&(U256::ZERO, U256::from(2))),
            "Storage change from 0 to 2 on slot 1"
        );
    }

    #[test]
    fn dao_hardfork_irregular_state_change() {
        let header = Header { number: 1, ..Header::default() };

        let mut db = StateProviderTest::default();

        let mut beneficiary_balance = 0;
        for (i, dao_address) in crate::eth_dao_fork::DAO_HARDKFORK_ACCOUNTS.iter().enumerate() {
            db.insert_account(
                *dao_address,
                Account { balance: U256::from(i), nonce: 0x00, bytecode_hash: None },
                None,
                HashMap::new(),
            );
            beneficiary_balance += i;
        }

        let chain_spec = ChainSpecBuilder::from(&*MAINNET)
            .homestead_activated()
            .with_fork(Hardfork::Dao, ForkCondition::Block(1))
            .build();

        let mut db = SubState::new(State::new(db));
        // execute chain and verify receipts
        let out = execute_and_verify_receipt(
            &Block { header, body: vec![], ommers: vec![] },
            U256::ZERO,
            None,
            &chain_spec,
            &mut db,
        )
        .unwrap();
        assert_eq!(out.changesets.len(), 0, "No tx");

        // Check if cache is set
        // beneficiary
        let dao_beneficiary =
            db.accounts.get(&crate::eth_dao_fork::DAO_HARDFORK_BENEFICIARY).unwrap();

        assert_eq!(dao_beneficiary.info.balance, U256::from(beneficiary_balance));
        for address in crate::eth_dao_fork::DAO_HARDKFORK_ACCOUNTS.iter() {
            let account = db.accounts.get(address).unwrap();
            assert_eq!(account.info.balance, U256::ZERO);
        }

        // check changesets
        let block_reward = out.block_reward.unwrap();
        let change_set = block_reward.get(&crate::eth_dao_fork::DAO_HARDFORK_BENEFICIARY).unwrap();
        assert_eq!(
            *change_set,
            AccountInfoChangeSet::Changed {
                new: Account { balance: U256::from(beneficiary_balance), ..Default::default() },
                old: Account { balance: U256::ZERO, ..Default::default() }
            }
        );
        for (i, address) in crate::eth_dao_fork::DAO_HARDKFORK_ACCOUNTS.iter().enumerate() {
            let change_set = block_reward.get(address).unwrap();
            assert_eq!(
                *change_set,
                AccountInfoChangeSet::Changed {
                    new: Account { balance: U256::ZERO, ..Default::default() },
                    old: Account { balance: U256::from(i), ..Default::default() }
                }
            );
        }
    }

    #[test]
    fn test_selfdestruct() {
        // Modified version of eth test. Storage is added for selfdestructed account to see
        // that changeset is set.
        // Got rlp block from: src/GeneralStateTestsFiller/stArgsZeroOneBalance/suicideNonConst.json

        let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        let mut db = StateProviderTest::default();

        let address_caller = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let address_selfdestruct = H160(hex!("095e7baea6a6c7c4c2dfeb977efac326af552d87"));

        // pre state
        let pre_account_caller = Account {
            balance: U256::from(0x0de0b6b3a7640000u64),
            nonce: 0x00,
            bytecode_hash: None,
        };

        db.insert_account(address_caller, pre_account_caller, None, HashMap::new());

        // insert account that will selfd

        let pre_account_selfdestroyed = Account {
            balance: U256::ZERO,
            nonce: 0x00,
            bytecode_hash: Some(H256(hex!(
                "56a7d44a4ecf086c34482ad1feb1007087fc56fae6dbefbd3f416002933f1705"
            ))),
        };

        let selfdestroyed_storage =
            BTreeMap::from([(H256::zero(), U256::ZERO), (H256::from_low_u64_be(1), U256::from(1))]);
        db.insert_account(
            address_selfdestruct,
            pre_account_selfdestroyed,
            Some(hex!("73095e7baea6a6c7c4c2dfeb977efac326af552d8731ff00").into()),
            selfdestroyed_storage.into_iter().collect::<HashMap<_, _>>(),
        );

        // spec at berlin fork
        let chain_spec = ChainSpecBuilder::mainnet().berlin_activated().build();

        let mut db = SubState::new(State::new(db));

        // execute chain and verify receipts
        let out =
            execute_and_verify_receipt(&block.unseal(), U256::ZERO, None, &chain_spec, &mut db)
                .unwrap();

        assert_eq!(out.changesets.len(), 1, "Should executed one transaction");

        let changesets = out.changesets[0].clone();
        assert_eq!(changesets.new_bytecodes.len(), 0, "Should have zero new bytecodes");

        let post_account_caller = Account {
            balance: U256::from(0x0de0b6b3a761cf60u64),
            nonce: 0x01,
            bytecode_hash: None,
        };

        assert_eq!(
            changesets.changeset.get(&address_caller).unwrap().account,
            AccountInfoChangeSet::Changed { new: post_account_caller, old: pre_account_caller },
            "Caller account has changed and fee is deduced"
        );

        let selfdestroyer_changeset = changesets.changeset.get(&address_selfdestruct).unwrap();

        // check account
        assert_eq!(
            selfdestroyer_changeset.account,
            AccountInfoChangeSet::Destroyed { old: pre_account_selfdestroyed },
            "Selfdestroyed account"
        );

        assert!(selfdestroyer_changeset.wipe_storage);
    }
}
