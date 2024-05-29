//! Module for executing [kona_derive::types::L2ExecutionPayload]s against [reth_revm::Evm].

use std::sync::Arc;
use reth_evm::ConfigureEvm;
use reth_payload_builder::database::CachedReads;
use reth_basic_payload_builder::*;
use reth_node_optimism::{OptimismPayloadBuilderAttributes, OptimismPayloadBuilder};
use reth_revm::InMemoryDB;
use reth_primitives::{Bytes, SealedBlockWithSenders, ChainSpec, B256};
use reth_transaction_pool::TransactionPool;
use tracing::debug;
use kona_derive::types::L2AttributesWithParent;

/// Executes an [L2ExecutionPayload] against the EVM.
pub async fn exec_payload(
    db: &mut InMemoryDB,
    attributes: L2AttributesWithParent,
    pool: impl TransactionPool,
    evm_config: impl ConfigureEvm,
    chain_spec: Arc<ChainSpec>,
) -> eyre::Result<B256> {
    let builder = OptimismPayloadBuilder::new(chain_spec, evm_config);
    let blockchain_db = db;
    let payload_config = PayloadConfig::new(
        /* Best Payload */,
        Bytes::default(),
        OptimismPayloadBuilderAttributes::try_new(
            /* Best Payload .hash() */,
            reth_rpc_types::engine::OptimismPayloadAttributes::try_new(
                payload_attributes: attributes,
                transactions: None,
                no_tx_pool: None,
                gas_limit: None,
            ),
        )?,
        chain
    );
    let args = BuildArguments::new(
        blockchain_db,
        pool,
        CachedReads::default(),
        payload_config,
        Cancelled::default(),
        None,
    );
    let hash: B256 = match builder.try_build(args)? {
        BuildOutcome::Better { payload, .. } => {
            let block = payload.block();
            debug!(target: "exex::kona", ?block, "Built new payload");

            let senders = block.senders().expect("sender recovery failed");
            let block_with_senders =
                SealedBlockWithSenders::new(block.clone(), senders).unwrap();

            block_with_senders.hash_slow()
        },
        _ => unreachable!("other outcomes are unreachable"),
    };
    Ok(hash)
}

// Execute a rollup block and return (block with recovered senders)[BlockWithSenders], (bundle
// state)[BundleState] and list of (receipts)[Receipt].
// pub async fn exec_block<Pool: TransactionPool>(
//     db: &mut Database,
//     pool: &Pool,
//     tx: &TransactionSigned,
//     header: &RollupContract::BlockHeader,
//     block_data: Bytes,
//     block_data_hash: B256,
// ) -> eyre::Result<(BlockWithSenders, BundleState, Vec<Receipt>, Vec<ExecutionResult>)> {
//     if header.rollupChainId != U256::from(CHAIN_ID) {
//         eyre::bail!("Invalid rollup chain ID")
//     }
//
//     // Construct header
//     let header = construct_header(db, header)?;
//
//     // Decode transactions
//     let transactions = decode_transactions(pool, tx, block_data, block_data_hash).await?;
//
//     // Configure EVM
//     let evm_config = EthEvmConfig::default();
//     let mut evm = configure_evm(&evm_config, db, &header);
//
//     // Execute transactions
//     let (executed_txs, receipts, results) = execute_transactions(&mut evm, &header, transactions)?;
//
//     // Construct block and recover senders
//     let block = Block { header, body: executed_txs, ..Default::default() }
//         .with_recovered_senders()
//         .ok_or_eyre("failed to recover senders")?;
//
//     let bundle = evm.db_mut().take_bundle();
//
//     Ok((block, bundle, receipts, results))
// }
