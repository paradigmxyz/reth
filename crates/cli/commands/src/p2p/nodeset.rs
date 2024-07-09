//! Node set related subcommand of P2P Debugging tool,

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use enr::Enr;
use reth_fs_util as fs;
use secp256k1::SecretKey;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

/// The arguments for the `reth p2p nodeset` command
#[derive(Parser, Debug)]
pub struct Command {
    #[clap(subcommand)]
    subcommand: Subcommands,
}

#[derive(Serialize, Deserialize, Debug)]
struct Record {
    seq: u64,
    record: Enr<SecretKey>,
    score: i64,
    firstResponse: DateTime<Utc>,
    lastResponse: DateTime<Utc>,
    lastCheck: DateTime<Utc>,
}

impl Command {
    /// Execute `p2p nodeset` command
    pub fn execute(self) -> eyre::Result<()> {
        match self.subcommand {
            Subcommands::Info { file } => {
                let content = fs::read_to_string(file)?;

                let nodes: HashMap<String, Record> =
                    serde_json::from_str(&content).expect("failed to deserialize json");

                println!("Set contains {} nodes", nodes.len());

                let mut keys = HashMap::new();

                for (_, node) in nodes.iter() {
                    for (key, _) in node.record.iter() {
                        *keys.entry(String::from_utf8_lossy(&key).to_string()).or_insert(0) += 1;
                    }
                }

                let max_key_len = keys.keys().map(|k| k.len()).max().unwrap_or(0);

                let mut result: Vec<_> = keys.iter().collect();

                result.sort_by(|a, b| a.0.cmp(b.0));

                for (key, count) in result {
                    let key_len = key.len();
                    let padding = " ".repeat(max_key_len - key_len);
                    println!("{}{}: {}", padding, key, count);
                }
            }
        }

        Ok(())
    }
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    /// Show statistics about a node set
    Info {
        /// The path of the JSON file used to store node set.
        #[arg(long, value_name = "FILE", default_value = "known-peers.json", verbatim_doc_comment)]
        file: PathBuf,
    },
    // TODO: implement `filter` subcommand
    // ref: https://github.com/ethereum/go-ethereum/blob/master/cmd/devp2p/nodesetcmd.go#L51
}
