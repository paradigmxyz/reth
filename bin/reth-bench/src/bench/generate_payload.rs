use alloy_consensus::{
    Block as PrimitiveBlock, BlockBody, Header as PrimitiveHeader,
    Transaction as PrimitiveTransaction,
};
use alloy_eips::{eip7702::SignedAuthorization, Encodable2718, Typed2718};
use alloy_primitives::{bytes::BufMut, Bytes, ChainId, TxKind, B256, U256};
use alloy_rpc_types::{
    AccessList, Block as RpcBlock, BlockTransactions, Transaction as EthRpcTransaction,
};
use alloy_rpc_types_engine::ExecutionPayload;
use clap::Parser;
use delegate::delegate;
use eyre::Result;
use op_alloy_rpc_types::Transaction as OpRpcTransaction;
use reth_cli_runner::CliContext;
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Command for generating an `engine_newPayload` request data from an RPC block.
///
/// This command takes a JSON block input (either from a file or stdin) and generates
/// an execution payload that can be used with the `engine_newPayloadV*` API.
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to the json file to parse. If not specified, stdin will be used.
    #[arg(short, long)]
    path: Option<String>,

    /// The engine RPC url to use.
    #[arg(short, long, conflicts_with = "raw")]
    rpc_url: Option<String>,

    /// The JWT secret to use.
    #[arg(short, long, conflicts_with = "raw")]
    jwt_secret: Option<String>,

    /// Output the raw JSON payload, instead of `cast` command. When used with stdin this
    /// can be very powerful, for example:
    ///
    /// `cast block latest -r https://reth-ethereum.ithaca.xyz/rpc --full -j | reth-bench generate-payload --raw | cast rpc --jwt-secret <JWT_SECRET> engine_newPayloadV3 --raw`
    #[arg(long)]
    raw: bool,

    #[arg(long, default_value_t = 3)]
    new_payload_version: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RpcTransaction {
    Ethereum(EthRpcTransaction),
    Optimism(OpRpcTransaction),
}

impl Typed2718 for RpcTransaction {
    delegate! {
        to match self {
            Self::Ethereum(tx) => tx,
            Self::Optimism(tx) => tx,
        } {
            fn ty(&self) -> u8;
        }
    }
}

impl PrimitiveTransaction for RpcTransaction {
    delegate! {
        to match self {
            Self::Ethereum(tx) => tx,
            Self::Optimism(tx) => tx,
        } {
            fn chain_id(&self) -> Option<ChainId>;
            fn nonce(&self) -> u64;
            fn gas_limit(&self) -> u64;
            fn gas_price(&self) -> Option<u128>;
            fn max_fee_per_gas(&self) -> u128;
            fn max_priority_fee_per_gas(&self) -> Option<u128>;
            fn max_fee_per_blob_gas(&self) -> Option<u128>;
            fn priority_fee_or_price(&self) -> u128;
            fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;
            fn is_dynamic_fee(&self) -> bool;
            fn kind(&self) -> TxKind;
            fn is_create(&self) -> bool;
            fn value(&self) -> U256;
            fn input(&self) -> &Bytes;
            fn access_list(&self) -> Option<&AccessList>;
            fn blob_versioned_hashes(&self) -> Option<&[B256]>;
            fn authorization_list(&self) -> Option<&[SignedAuthorization]>;
        }
    }
}

impl Encodable2718 for RpcTransaction {
    delegate! {
        to match self {
            Self::Ethereum(tx) => tx.inner,
            Self::Optimism(tx) => tx.inner.inner,
        } {
            fn encode_2718_len(&self) -> usize;
            fn encode_2718(&self, out: &mut dyn BufMut);
        }
    }
}

impl Command {
    /// Execute the generate payload command
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        // Read from file or stdin
        let block_json = if let Some(path) = &self.path {
            std::fs::read_to_string(path)?
        } else {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            buffer
        };

        // Parse the block
        let block: RpcBlock<RpcTransaction> = serde_json::from_str(&block_json)?;

        // Extract parent beacon block root
        let parent_beacon_block_root = block.header.parent_beacon_block_root;

        // Extract transactions
        let transactions = match block.transactions {
            BlockTransactions::Hashes(_) => {
                return Err(eyre::eyre!("Block must include full transaction data. Send the eth_getBlockByHash request with full: `true`"));
            }
            BlockTransactions::Full(txs) => txs,
            BlockTransactions::Uncle => {
                return Err(eyre::eyre!("Cannot process uncle blocks"));
            }
        };

        // Extract blob versioned hashes
        let blob_versioned_hashes = transactions
            .iter()
            .filter_map(|tx| tx.blob_versioned_hashes().map(|v| v.to_vec()))
            .collect::<Vec<_>>();

        // Convert to execution payload
        let execution_payload = ExecutionPayload::from_block_slow(&PrimitiveBlock::new(
            PrimitiveHeader::from(block.header),
            BlockBody { transactions, ommers: vec![], withdrawals: block.withdrawals },
        ))
        .0;

        // Create JSON request data
        let json_request = serde_json::to_string(&(
            execution_payload,
            blob_versioned_hashes,
            parent_beacon_block_root,
        ))?;

        // Print output
        if self.raw {
            println!("{json_request}");
        } else {
            let mut cmd = format!(
                "cast rpc engine_newPayloadV{} --raw {}",
                self.new_payload_version, json_request
            );

            if let Some(rpc_url) = self.rpc_url {
                cmd += &format!(" --rpc-url {}", rpc_url);
            }
            if let Some(secret) = self.jwt_secret {
                cmd += &format!(" --jwt-secret {}", secret);
            }

            println!("{cmd}");
        }

        Ok(())
    }
}
