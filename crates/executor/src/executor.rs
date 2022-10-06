#![allow(missing_debug_implementations)]
use crate::Config;
use reth_interfaces::{
    consensus::Consensus,
    executor::{BlockExecutor, Error, ExecutorDb},
};
use reth_primitives::{Block, BlockLocked, Transaction, U256};
use revm::{
    db::{CacheDB, EmptyDB},
    AnalysisKind, BlockEnv, CreateScheme, Env, SpecId, TransactTo, TxEnv,
};

type TempStateDb = CacheDB<EmptyDB>;

/// Main block executor
pub struct Executor {
    /// Configuration, Spec and optional flags.
    config: Config,
    /// Database
    db: Box<dyn ExecutorDb>,
    /// Consensus
    consensus: Box<dyn Consensus>,
}

impl Executor {
    /// Create new Executor
    pub fn new(config: Config, db: Box<dyn ExecutorDb>, consensus: Box<dyn Consensus>) -> Self {
        Self { config, db, consensus }
    }

    /// Verify block. Execute all transaction and compare results.
    pub fn verify(&self, block: &BlockLocked) -> Result<(), Error> {
        let mut env = Env::default();
        env.cfg.chain_id = 1.into();
        env.cfg.spec_id = SpecId::LATEST;
        env.cfg.perf_all_precompiles_have_balance = true;
        env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

        fill_block_env(&mut env.block, block);

        let _database = TempStateDb::new(EmptyDB::default());

        for transaction in block.body.iter() {
            fill_tx_env(&mut env.tx, transaction.as_ref());
        }

        Err(Error::VerificationFailed)
    }
}

fn fill_block_env(block_env: &mut BlockEnv, block: &BlockLocked) {
    block_env.number = block.header.number.into();
    block_env.coinbase = block.header.beneficiary;
    block_env.timestamp = block.header.timestamp.into();
    block_env.difficulty = block.header.difficulty.into();
    block_env.basefee = block.header.base_fee_per_gas.unwrap_or_default().into();
    block_env.gas_limit = block.header.gas_limit.into();
}

fn fill_tx_env(tx_env: &mut TxEnv, transaction: &Transaction) {
    match transaction {
        Transaction::Legacy { nonce, chain_id, gas_price, gas_limit, to, value, input } => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = (*gas_price).into();
            tx_env.gas_priority_fee = None;
            tx_env.transact_to =
                if let Some(to) = to { TransactTo::Call(*to) } else { TransactTo::create() };
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
            tx_env.transact_to =
                if let Some(to) = to { TransactTo::Call(*to) } else { TransactTo::create() };
            tx_env.value = *value;
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
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
            tx_env.transact_to =
                if let Some(to) = to { TransactTo::Call(*to) } else { TransactTo::create() };
            tx_env.value = *value;
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
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

impl BlockExecutor for Executor {}
