use reth_interfaces::Error;
use reth_primitives::{
    Account, Header, Transaction, TransactionKind, TransactionSignedEcRecovered, TxEip1559,
    TxEip2930, TxLegacy, H160, H256, KECCAK_EMPTY, U256,
};
use reth_provider::StateProvider;
use revm::{
    db::{CacheDB, DatabaseRef},
    BlockEnv, TransactTo, TxEnv,
};

/// SubState of database. Uses revm internal cache with binding to reth StateProvider trait.
pub type SubState<DB> = CacheDB<State<DB>>;

/// Wrapper around StateProvider that implements revm database trait
pub struct State<DB: StateProvider>(pub DB);

impl<DB: StateProvider> State<DB> {
    /// Create new State with generic StateProvider.
    pub fn new(db: DB) -> Self {
        Self(db)
    }

    /// Return inner state reference
    pub fn state(&self) -> &DB {
        &self.0
    }

    /// Return inner state mutable reference
    pub fn state_mut(&mut self) -> &mut DB {
        &mut self.0
    }

    /// Consume State and return inner StateProvider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB: StateProvider> DatabaseRef for State<DB> {
    type Error = Error;

    fn basic(&self, address: H160) -> Result<Option<revm::AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address)?.map(|account| revm::AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
        }))
    }

    fn code_by_hash(&self, code_hash: H256) -> Result<revm::Bytecode, Self::Error> {
        let bytecode = self.0.bytecode_by_hash(code_hash)?.unwrap_or_default();
        Ok(revm::Bytecode::new_raw(bytecode.0))
    }

    fn storage(&self, address: H160, index: U256) -> Result<U256, Self::Error> {
        let index = H256(index.to_be_bytes());
        let ret = self.0.storage(address, index)?.unwrap_or_default();
        Ok(ret)
    }

    fn block_hash(&self, number: U256) -> Result<H256, Self::Error> {
        Ok(self.0.block_hash(number)?.unwrap_or_default())
    }
}

/// Fill block environment from Block.
pub fn fill_block_env(block_env: &mut BlockEnv, header: &Header, after_merge: bool) {
    block_env.number = U256::from(header.number);
    block_env.coinbase = header.beneficiary;
    block_env.timestamp = U256::from(header.timestamp);
    if after_merge {
        block_env.prevrandao = Some(header.mix_hash);
        block_env.difficulty = U256::ZERO;
    } else {
        block_env.difficulty = header.difficulty;
        block_env.prevrandao = None;
    }
    block_env.basefee = U256::from(header.base_fee_per_gas.unwrap_or_default());
    block_env.gas_limit = U256::from(header.gas_limit);
}

/// Fill transaction environment from Transaction.
pub fn fill_tx_env(tx_env: &mut TxEnv, transaction: &TransactionSignedEcRecovered) {
    tx_env.caller = transaction.signer();
    match transaction.as_ref().as_ref() {
        Transaction::Legacy(TxLegacy {
            nonce,
            chain_id,
            gas_price,
            gas_limit,
            to,
            value,
            input,
        }) => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = U256::from(*gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = U256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = *chain_id;
            tx_env.nonce = Some(*nonce);
        }
        Transaction::Eip2930(TxEip2930 {
            nonce,
            chain_id,
            gas_price,
            gas_limit,
            to,
            value,
            input,
            access_list,
        }) => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = U256::from(*gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = U256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        l.address,
                        l.storage_keys
                            .iter()
                            .map(|k| U256::from_be_bytes(k.to_fixed_bytes()))
                            .collect(),
                    )
                })
                .collect();
        }
        Transaction::Eip1559(TxEip1559 {
            nonce,
            chain_id,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            input,
            access_list,
        }) => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = U256::from(*max_fee_per_gas);
            tx_env.gas_priority_fee = Some(U256::from(*max_priority_fee_per_gas));
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = U256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        l.address,
                        l.storage_keys
                            .iter()
                            .map(|k| U256::from_be_bytes(k.to_fixed_bytes()))
                            .collect(),
                    )
                })
                .collect();
        }
    }
}

/// Check equality between [`reth_primitives::Log`] and [`revm::Log`]
pub fn is_log_equal(revm_log: &revm::Log, reth_log: &reth_primitives::Log) -> bool {
    revm_log.topics.len() == reth_log.topics.len() &&
        revm_log.address.0 == reth_log.address.0 &&
        revm_log.data == reth_log.data &&
        !revm_log
            .topics
            .iter()
            .zip(reth_log.topics.iter())
            .any(|(revm_topic, reth_topic)| revm_topic.0 != reth_topic.0)
}

/// Create reth primitive [Account] from [revm::AccountInfo].
/// Check if revm bytecode hash is [KECCAK_EMPTY] and put None to reth [Account]
pub fn to_reth_acc(revm_acc: &revm::AccountInfo) -> Account {
    let code_hash = revm_acc.code_hash;
    Account {
        balance: revm_acc.balance,
        nonce: revm_acc.nonce,
        bytecode_hash: if code_hash == KECCAK_EMPTY { None } else { Some(code_hash) },
    }
}
