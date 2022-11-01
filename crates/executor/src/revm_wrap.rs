use reth_interfaces::executor::ExecutorDb;
use reth_primitives::{BlockLocked, Transaction, TransactionKind, H160, H256, U256};
use revm::{
    db::{CacheDB, DatabaseRef},
    BlockEnv, TransactTo, TxEnv,
};
use std::convert::Infallible;

/// SubState of database. Uses revm internal cache with binding to reth DbExecutor trait.
pub type SubState<DB> = CacheDB<State<DB>>;

/// Wrapper around ExeuctorDb that implements revm database trait
pub struct State<DB: ExecutorDb>(DB);

impl<DB: ExecutorDb> State<DB> {
    /// Create new State with generic ExecutorDb.
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

    /// Consume State and return inner DbExecutable.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB: ExecutorDb> DatabaseRef for State<DB> {
    type Error = Infallible;

    fn basic(&self, address: H160) -> Result<Option<revm::AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address).map(|account| revm::AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash,
            code: None,
        }))
    }

    fn code_by_hash(&self, code_hash: H256) -> Result<revm::Bytecode, Self::Error> {
        let (bytecode, size) = self.0.bytecode_by_hash(code_hash).unwrap_or_default();
        Ok(unsafe { revm::Bytecode::new_checked(bytecode.0, size, Some(code_hash)) })
    }

    fn storage(&self, address: H160, index: U256) -> Result<U256, Self::Error> {
        let mut h_index = H256::zero();
        index.to_big_endian(h_index.as_bytes_mut());

        Ok(U256::from_big_endian(self.0.storage(address, h_index).unwrap_or_default().as_ref()))
    }

    fn block_hash(&self, number: U256) -> Result<H256, Self::Error> {
        Ok(self.0.block_hash(number).unwrap_or_default())
    }
}

/// Fill block environment from Block.
pub fn fill_block_env(block_env: &mut BlockEnv, block: &BlockLocked) {
    block_env.number = block.header.number.into();
    block_env.coinbase = block.header.beneficiary;
    block_env.timestamp = block.header.timestamp.into();
    block_env.difficulty = block.header.difficulty;
    block_env.basefee = block.header.base_fee_per_gas.unwrap_or_default().into();
    block_env.gas_limit = block.header.gas_limit.into();
}

/// Fill transaction environment from Transaction.
pub fn fill_tx_env(tx_env: &mut TxEnv, transaction: &Transaction) {
    match transaction {
        Transaction::Legacy { nonce, chain_id, gas_price, gas_limit, to, value, input } => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = (*gas_price).into();
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = *value;
            tx_env.data = input.0.clone();
            tx_env.chain_id = *chain_id;
            tx_env.nonce = Some(*nonce);
        }
        Transaction::Eip2930 {
            nonce,
            chain_id,
            gas_price,
            gas_limit,
            to,
            value,
            input,
            access_list,
        } => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = (*gas_price).into();
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = *value;
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        l.address,
                        l.storage_keys.iter().map(|k| U256::from_big_endian(k.as_ref())).collect(),
                    )
                })
                .collect();
        }
        Transaction::Eip1559 {
            nonce,
            chain_id,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            input,
            access_list,
        } => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = (*max_fee_per_gas).into();
            tx_env.gas_priority_fee = Some((*max_priority_fee_per_gas).into());
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = *value;
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        l.address,
                        l.storage_keys.iter().map(|k| U256::from_big_endian(k.as_ref())).collect(),
                    )
                })
                .collect();
        }
    }
}
