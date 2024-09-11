use alloy_primitives::B256;
use reth_chainspec::ChainSpec;
use reth_db::{models::StoredBlockBodyIndices, tables, DatabaseEnv};
use reth_db_api::transaction::DbTxMut;
use reth_node_builder::{NodeTypesWithDBAdapter, NodeTypesWithEngine};
use reth_optimism_primitives::ovm::{LAST_OVM_HEADER, LAST_OVM_HEADER_HASH, LAST_OVM_HEADER_TTD};
use reth_primitives::{Header, StaticFileSegment, U256};
use reth_provider::{
    providers::StaticFileProvider, writer::UnifiedStorageWriter, ProviderFactory,
    StageCheckpointWriter, StaticFileProviderFactory, StaticFileWriter,
};
use reth_stages::{StageCheckpoint, StageId};
use std::sync::Arc;
use tracing::info;

/// Creates a dummy chain (with no transactions) up to the last OVM block inclusive.
///
/// The last OVM header needs to be valid so the node can validate the parent header of the first
/// bedrock header.
pub(crate) fn setup_op_mainnet_without_ovm<N: NodeTypesWithEngine<ChainSpec = ChainSpec>>(
    provider_factory: ProviderFactory<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
) -> Result<(), eyre::Error> {
    info!(target: "reth::cli", "Setting up dummy OVM chain before importing state.");

    // Write OVM dummy data up to `LAST_OVM_HEADER - 1` block
    insert_dummy_chain(provider_factory.static_file_provider())?;

    info!(target: "reth::cli", "Inserting last OVM header.");

    // Insert last OVM header
    insert_last_ovm_header(provider_factory.static_file_provider())?;

    // Writes `BlockBodyIndices` and  `StageCheckpoint`
    let mut provider_rw = provider_factory.provider_rw()?;
    {
        provider_rw.tx_mut().put::<tables::BlockBodyIndices>(
            LAST_OVM_HEADER.number,
            StoredBlockBodyIndices { first_tx_num: 0, tx_count: 0 },
        )?;

        for stage in StageId::ALL {
            provider_rw
                .save_stage_checkpoint(stage, StageCheckpoint::new(LAST_OVM_HEADER.number))?;
        }
    }

    UnifiedStorageWriter::commit(provider_rw, provider_factory.static_file_provider())?;

    info!(target: "reth::cli", "Set up finished.");

    Ok(())
}

/// Appends the last ovm header. This is necessary for appending the first bedrock block during
/// sync.
///
/// By appending it, static file writer also verifies that all segments are at the same
/// height.
fn insert_last_ovm_header(sf_provider: StaticFileProvider) -> Result<(), eyre::Error> {
    sf_provider.latest_writer(StaticFileSegment::Headers)?.append_header(
        &LAST_OVM_HEADER,
        LAST_OVM_HEADER_TTD,
        &LAST_OVM_HEADER_HASH,
    )?;

    sf_provider
        .latest_writer(StaticFileSegment::Receipts)?
        .increment_block(LAST_OVM_HEADER.number)?;

    sf_provider
        .latest_writer(StaticFileSegment::Transactions)?
        .increment_block(LAST_OVM_HEADER.number)?;

    Ok(())
}

/// Creates a dummy chain with no transactions/receipts up to `LAST_OVM_HEADER - 1` block.
///
/// * Headers: It will push an empty block.
/// * Transactions: It will not push any tx, only increments the end block range.
/// * Receipts: It will not push any receipt, only increments the end block range.
fn insert_dummy_chain(sf_provider: StaticFileProvider) -> Result<(), eyre::Error> {
    let mut headers_writer = sf_provider.latest_writer(StaticFileSegment::Headers)?;
    let txs_writer = sf_provider.latest_writer(StaticFileSegment::Transactions)?;
    let receipts_writer = sf_provider.latest_writer(StaticFileSegment::Receipts)?;
    let (tx, rx) = std::sync::mpsc::channel();

    rayon::scope(|s| {
        // Spawn jobs for incrementing the block end range of transactions and receipts
        for mut writer in [txs_writer, receipts_writer] {
            let tx_clone = tx.clone();
            s.spawn(move |_| {
                for block_num in 1..LAST_OVM_HEADER.number {
                    if let Err(e) = writer.increment_block(block_num) {
                        tx_clone.send(Err(e)).unwrap();
                        return;
                    }
                }
                tx_clone.send(Ok(())).unwrap();
            });
        }

        // Spawn job for appending empty headers
        s.spawn(move |_| {
            let mut empty_header = Header::default();
            // TODO: should we fill with real parent_hash?
            for block_num in 1..LAST_OVM_HEADER.number {
                empty_header.number = block_num;
                if let Err(e) = headers_writer.append_header(&empty_header, U256::ZERO, &B256::ZERO)
                {
                    tx.send(Err(e)).unwrap();
                    return;
                }
            }
            tx.send(Ok(())).unwrap();
        });
    });

    // Catches any StaticFileWriter error.
    while let Ok(r) = rx.recv() {
        r?;
    }

    // If, for any reason, rayon crashes this verifies if all segments are at the same height.
    for segment in
        [StaticFileSegment::Headers, StaticFileSegment::Receipts, StaticFileSegment::Transactions]
    {
        assert_eq!(
            sf_provider.latest_writer(segment)?.user_header().block_end(),
            Some(LAST_OVM_HEADER.number - 1)
        );
    }

    Ok(())
}
