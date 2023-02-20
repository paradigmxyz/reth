#![allow(missing_docs)]
//! Main `t8n` command
//!
//! Runs an EVM state transition using Reth's executor module

mod provider;
use provider::*;

use reth_executor::{
    executor::{test_utils::InMemoryStateProvider, Executor},
    revm_wrap::{State, SubState},
};
use reth_primitives::{Block, ChainSpecBuilder, Hardfork, Header, U256};
use reth_rpc_types as rpc;

use clap::Parser;

use std::{collections::BTreeMap, fs::File, path::PathBuf};

/// `reth t8n` command
#[derive(Debug, Parser)]
pub struct Command {
    #[arg(long = "input.alloc")]
    alloc: PathBuf,
    #[arg(long = "input.env")]
    env: PathBuf,
    #[arg(long = "input.txs")]
    txs: PathBuf,
    #[arg(long)]
    trace: bool,
    #[arg(long = "state.fork")]
    fork: Hardfork,
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let prestate = File::open(&self.alloc)?;
        let prestate: BTreeMap<reth_primitives::Address, PrestateAccount> =
            serde_json::from_reader(prestate)?;

        let env = File::open(&self.env)?;
        let env: PrestateEnv = serde_json::from_reader(env)?;

        let txs = File::open(&self.txs)?;
        let txs: Vec<rpc::Transaction> = serde_json::from_reader(txs)?;

        let mut db = InMemoryStateProvider::default();
        for (address, account) in prestate {
            let reth_account = reth_primitives::Account {
                nonce: account.nonce.as_u64(),
                balance: account.balance,
                bytecode_hash: None, // this gets set inside the `insert_account` method
            };
            db.insert_account(address, reth_account, account.code, account.storage);
        }

        let spec = ChainSpecBuilder::mainnet().build();
        let db = State::new(db);
        let mut db = SubState::new(db);
        let block = Block {
            header: Header {
                beneficiary: env.current_coinbase,
                // TODO: Make RANDAO-aware for post-Shanghai blocks
                difficulty: env.current_difficulty,
                number: env.current_number.as_u64(),
                timestamp: env.current_timestamp.to::<u64>(),
                gas_limit: env.current_gas_limit.to::<u64>(),
                ..Default::default()
            },
            body: txs.into_iter().map(|x| x.into_transaction()).collect(),
            ..Default::default()
        };
        let mut executor = Executor::new(&spec, &mut db);
        let result = executor.execute_transactions(&block, U256::ZERO, None);

        // State is committed, so we can try calculating stateroot, txs root etc.
        dbg!(&result);

        Ok(())
    }
}
