#![allow(missing_docs)]
//! Main `t8n` command
//!
//! Runs an EVM state transition using Reth's executor module

mod provider;
use provider::*;

use reth_executor::executor::Executor;
use reth_primitives::Hardfork;

use ethers_core::types::{Address, Bytes, Transaction, U256, U64};

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
        let _prestate: BTreeMap<Address, PrestateAccount> = serde_json::from_reader(prestate)?;

        let env = File::open(&self.env)?;
        let _env: PrestateEnv = serde_json::from_reader(env)?;

        let txs = File::open(&self.txs)?;
        let _txs: BTreeMap<Address, PrestateAccount> = serde_json::from_reader(txs)?;

        // 1. Instantiate the DB with the pre-state
        // 2. Instantiate the executor
        // 3. Run it
        // 4. Diff against the actual one

        Ok(())
    }
}
