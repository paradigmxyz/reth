use reth_interfaces::Error;
use reth_primitives::{
    Account, Header, Transaction, TransactionKind, TransactionSignedEcRecovered, TxEip1559,
    TxEip2930, TxLegacy, H160, H256, KECCAK_EMPTY, U256,
};
use reth_provider::StateProvider;
use revm::{
    db::{CacheDB, DatabaseRef},
    BlockEnv, TransactTo, TxEnv, B160, B256, U256 as evmU256,
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

    fn basic(&self, address: B160) -> Result<Option<revm::AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(H160(address.0))?.map(|account| revm::AccountInfo {
            balance: evmU256::from_limbs(account.balance.0),
            nonce: account.nonce,
            code_hash: B256(account.bytecode_hash.unwrap_or(KECCAK_EMPTY).0),
            code: None,
        }))
    }

    fn code_by_hash(&self, code_hash: B256) -> Result<revm::Bytecode, Self::Error> {
        let bytecode = self.0.bytecode_by_hash(H256(code_hash.0))?.unwrap_or_default();
        Ok(revm::Bytecode::new_raw(bytecode.0))
    }

    fn storage(&self, address: B160, index: evmU256) -> Result<evmU256, Self::Error> {
        let index = H256(index.to_be_bytes());
        let ret =
            evmU256::from_limbs(self.0.storage(H160(address.0), index)?.unwrap_or_default().0);
        Ok(ret)
    }

    fn block_hash(&self, number: evmU256) -> Result<B256, Self::Error> {
        Ok(B256(self.0.block_hash(U256(*number.as_limbs()))?.unwrap_or_default().0))
    }
}

/// Fill block environment from Block.
pub fn fill_block_env(block_env: &mut BlockEnv, header: &Header) {
    block_env.number = evmU256::from(header.number);
    block_env.coinbase = B160(header.beneficiary.0);
    block_env.timestamp = evmU256::from(header.timestamp);
    block_env.difficulty = evmU256::from_limbs(header.difficulty.0);
    block_env.basefee = evmU256::from(header.base_fee_per_gas.unwrap_or_default());
    block_env.gas_limit = evmU256::from(header.gas_limit);
}

/// Fill transaction environment from Transaction.
pub fn fill_tx_env(tx_env: &mut TxEnv, transaction: &TransactionSignedEcRecovered) {
    tx_env.caller = B160(transaction.signer().0);
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
            tx_env.gas_price = evmU256::from(*gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(B160(to.0)),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = evmU256::from(*value);
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
            tx_env.gas_price = evmU256::from(*gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(B160(to.0)),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = evmU256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        B160(l.address.0),
                        l.storage_keys
                            .iter()
                            .map(|k| evmU256::from_be_bytes(k.to_fixed_bytes()))
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
            tx_env.gas_price = evmU256::from(*max_fee_per_gas);
            tx_env.gas_priority_fee = Some(evmU256::from(*max_priority_fee_per_gas));
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(B160(to.0)),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = evmU256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        B160(l.address.0),
                        l.storage_keys
                            .iter()
                            .map(|k| evmU256::from_be_bytes(k.to_fixed_bytes()))
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
    let code_hash = H256(revm_acc.code_hash.0);
    Account {
        balance: U256(*revm_acc.balance.as_limbs()),
        nonce: revm_acc.nonce,
        bytecode_hash: if code_hash == KECCAK_EMPTY { None } else { Some(code_hash) },
    }
}
