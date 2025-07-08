//! Utilities for creating and writing RLP test data

use alloy_consensus::{constants::EMPTY_WITHDRAWALS, BlockHeader, Header};
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Address, B256, B64, U256};
use alloy_rlp::Encodable;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_ethereum_primitives::{Block, BlockBody};
use reth_primitives::SealedBlock;
use reth_primitives_traits::Block as BlockTrait;
use std::{io::Write, path::Path};
use tracing::debug;

/// Generate test blocks for a given chain spec
pub fn generate_test_blocks(chain_spec: &ChainSpec, count: u64) -> Vec<SealedBlock> {
    let mut blocks: Vec<SealedBlock> = Vec::new();
    let genesis_header = chain_spec.sealed_genesis_header();
    let mut parent_hash = genesis_header.hash();
    let mut parent_number = genesis_header.number();
    let mut parent_base_fee = genesis_header.base_fee_per_gas;
    let mut parent_gas_limit = genesis_header.gas_limit;

    debug!(target: "e2e::import",
        "Genesis header base fee: {:?}, gas limit: {}, state root: {:?}",
        parent_base_fee,
        parent_gas_limit,
        genesis_header.state_root()
    );

    for i in 1..=count {
        // Create a simple header
        let mut header = Header {
            parent_hash,
            number: parent_number + 1,
            gas_limit: parent_gas_limit, // Use parent's gas limit
            gas_used: 0,                 // Empty blocks use no gas
            timestamp: genesis_header.timestamp() + i * 12, // 12 second blocks
            beneficiary: Address::ZERO,
            receipts_root: alloy_consensus::constants::EMPTY_RECEIPTS,
            logs_bloom: Default::default(),
            difficulty: U256::from(1), // Will be overridden for post-merge
            // Use the same state root as parent for now (empty state changes)
            state_root: if i == 1 {
                genesis_header.state_root()
            } else {
                blocks.last().unwrap().state_root
            },
            transactions_root: alloy_consensus::constants::EMPTY_TRANSACTIONS,
            ommers_hash: alloy_consensus::constants::EMPTY_OMMER_ROOT_HASH,
            mix_hash: B256::ZERO,
            nonce: B64::from(0u64),
            extra_data: Default::default(),
            base_fee_per_gas: None,
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
        };

        // Set required fields based on chain spec
        if chain_spec.is_london_active_at_block(header.number) {
            // Calculate base fee based on parent block
            if let Some(parent_fee) = parent_base_fee {
                // For the first block, we need to use the exact expected base fee
                // The consensus rules expect it to be calculated from the genesis
                let (parent_gas_used, parent_gas_limit) = if i == 1 {
                    // Genesis block parameters
                    (genesis_header.gas_used, genesis_header.gas_limit)
                } else {
                    let last_block = blocks.last().unwrap();
                    (last_block.gas_used, last_block.gas_limit)
                };
                header.base_fee_per_gas = Some(alloy_eips::calc_next_block_base_fee(
                    parent_gas_used,
                    parent_gas_limit,
                    parent_fee,
                    chain_spec.base_fee_params_at_timestamp(header.timestamp),
                ));
                debug!(target: "e2e::import", "Block {} calculated base fee: {:?} (parent gas used: {}, parent gas limit: {}, parent base fee: {})",
                    i, header.base_fee_per_gas, parent_gas_used, parent_gas_limit, parent_fee);
                parent_base_fee = header.base_fee_per_gas;
            }
        }

        // For post-merge blocks
        if chain_spec.is_paris_active_at_block(header.number) {
            header.difficulty = U256::ZERO;
            header.nonce = B64::ZERO;
        }

        // For post-shanghai blocks
        if chain_spec.is_shanghai_active_at_timestamp(header.timestamp) {
            header.withdrawals_root = Some(EMPTY_WITHDRAWALS);
        }

        // For post-cancun blocks
        if chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            header.blob_gas_used = Some(0);
            header.excess_blob_gas = Some(0);
            header.parent_beacon_block_root = Some(B256::ZERO);
        }

        // Create an empty block body
        let body = BlockBody {
            transactions: vec![],
            ommers: vec![],
            withdrawals: header.withdrawals_root.is_some().then(Withdrawals::default),
        };

        // Create the block
        let block = Block { header: header.clone(), body: body.clone() };
        let sealed_block = BlockTrait::seal_slow(block);

        debug!(target: "e2e::import",
            "Generated block {} with hash {:?}",
            sealed_block.number(),
            sealed_block.hash()
        );
        debug!(target: "e2e::import",
            "  Body has {} transactions, {} ommers, withdrawals: {}",
            body.transactions.len(),
            body.ommers.len(),
            body.withdrawals.is_some()
        );

        // Update parent for next iteration
        parent_hash = sealed_block.hash();
        parent_number = sealed_block.number();
        parent_gas_limit = sealed_block.gas_limit;
        if header.base_fee_per_gas.is_some() {
            parent_base_fee = header.base_fee_per_gas;
        }

        blocks.push(sealed_block);
    }

    blocks
}

/// Write blocks to RLP file
pub fn write_blocks_to_rlp(blocks: &[SealedBlock], path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    let mut total_bytes = 0;

    for (i, block) in blocks.iter().enumerate() {
        // Convert SealedBlock to Block before encoding
        let block_for_encoding = block.clone().unseal();

        let mut buf = Vec::new();
        block_for_encoding.encode(&mut buf);
        debug!(target: "e2e::import",
            "Block {} has {} transactions, encoded to {} bytes",
            i,
            block.body().transactions.len(),
            buf.len()
        );

        // Debug: check what's in the encoded data
        debug!(target: "e2e::import", "Block {} encoded to {} bytes", i, buf.len());
        if buf.len() < 20 {
            debug!(target: "e2e::import", "  Raw bytes: {:?}", &buf);
        } else {
            debug!(target: "e2e::import", "  First 20 bytes: {:?}", &buf[..20]);
        }

        total_bytes += buf.len();
        file.write_all(&buf)?;
    }

    file.flush()?;
    debug!(target: "e2e::import", "Total RLP bytes written: {total_bytes}");
    Ok(())
}

/// Create FCU JSON for the tip of the chain
pub fn create_fcu_json(tip: &SealedBlock) -> serde_json::Value {
    serde_json::json!({
        "params": [{
            "headBlockHash": format!("0x{:x}", tip.hash()),
            "safeBlockHash": format!("0x{:x}", tip.hash()),
            "finalizedBlockHash": format!("0x{:x}", tip.hash()),
        }]
    })
}
