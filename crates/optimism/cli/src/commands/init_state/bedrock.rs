use alloy_primitives::B256;
use reth_db::{models::StoredBlockBodyIndices, tables, Database};
use reth_db_api::transaction::DbTxMut;
use reth_optimism_primitives::bedrock::{BEDROCK_HEADER, BEDROCK_HEADER_HASH, BEDROCK_HEADER_TTD};
use reth_primitives::{Header, StaticFileSegment, U256};
use reth_provider::{
    providers::StaticFileProvider, DatabaseProviderRW, StageCheckpointWriter,
    StaticFileProviderFactory, StaticFileWriter,
};
use reth_stages::{StageCheckpoint, StageId};
use tracing::info;

/// Creates a dummy chain (with no transactions) up to the last OVM block . It then, appends the first valid Bedrock block.
pub(crate) fn setup_op_mainnet_without_ovm<DB: Database>(
    provider_rw: &DatabaseProviderRW<DB>,
) -> Result<(), eyre::Error> {
    info!(target: "reth::cli", "Setting up dummy OVM chain before importing state.");

    // Write OVM dummy data up to `BEDROCK_HEADER - 1` block
    insert_dummy_chain(provider_rw.static_file_provider())?;

    info!(target: "reth::cli", "Inserting first Bedrock header.");

    insert_first_bedrock_header(provider_rw.static_file_provider())?;

    // Writes `BlockBodyIndices` and  `StageCheckpoint`
    {
        provider_rw.tx_ref().put::<tables::BlockBodyIndices>(
            BEDROCK_HEADER.number,
            StoredBlockBodyIndices { first_tx_num: 0, tx_count: 0 },
        )?;

        for stage in StageId::ALL {
            provider_rw
                .save_stage_checkpoint(stage, StageCheckpoint::new(BEDROCK_HEADER.number))?;
        }
    }

    info!(target: "reth::cli", "Set up finished.");

    Ok(())
}

/// Appends the first bedrock header.
///
/// By appending it, static file writer also verifies that all segments are at the same
/// height.
fn insert_first_bedrock_header(sf_provider: &StaticFileProvider) -> Result<(), eyre::Error> {
    sf_provider.latest_writer(StaticFileSegment::Headers)?.append_header(
        &BEDROCK_HEADER,
        BEDROCK_HEADER_TTD,
        &BEDROCK_HEADER_HASH,
    )?;

    sf_provider
        .latest_writer(StaticFileSegment::Receipts)?
        .increment_block(BEDROCK_HEADER.number)?;

    sf_provider
        .latest_writer(StaticFileSegment::Transactions)?
        .increment_block(BEDROCK_HEADER.number)?;

    Ok(())
}

/// Creates a dummy chain with no transactions/receipts up to `BEDROCK_HEADER - 1` block.
///
/// * Headers: It will push an empty block.
/// * Transactions: It will not push any tx, only increments the end block range.
/// * Receipts: It will not push any receipt, only increments the end block range.
fn insert_dummy_chain(sf_provider: &StaticFileProvider) -> Result<(), eyre::Error> {
    let mut headers_writer = sf_provider.latest_writer(StaticFileSegment::Headers)?;
    let txs_writer = sf_provider.latest_writer(StaticFileSegment::Transactions)?;
    let receipts_writer = sf_provider.latest_writer(StaticFileSegment::Receipts)?;
    let (tx, rx) = std::sync::mpsc::channel();

    rayon::scope(|s| {
        // Spawn jobs for incrementing the block end range of transactions and receipts
        for mut writer in [txs_writer, receipts_writer] {
            let tx_clone = tx.clone();
            s.spawn(move |_| {
                for block_num in 1..BEDROCK_HEADER.number {
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
            for block_num in 1..BEDROCK_HEADER.number {
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
            Some(BEDROCK_HEADER.number - 1)
        );
    }

    Ok(())
}
