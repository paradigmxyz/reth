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
use reth_primitives::{Account, ChainSpec, ChainSpecBuilder, Hardfork};

use clap::Parser;
use serde::{Deserialize, Serialize};

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
        let _env: PrestateEnv = serde_json::from_reader(env)?;

        let txs = File::open(&self.txs)?;
        // todo: ethers-core transaction?
        let _txs: Vec<reth_primitives::Transaction> = serde_json::from_reader(txs)?;

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
        let _executor = Executor::new(&spec, &mut db);

        // TODO: Construct the header etc.

        Ok(())
    }
}
