use alloy_primitives::{BlockNumber, B256, U256};
use reth_optimism_primitives::bedrock::{BEDROCK_HEADER, BEDROCK_HEADER_HASH, BEDROCK_HEADER_TTD};
use reth_primitives::{
    BlockBody, Header, SealedBlock, SealedBlockWithSenders, SealedHeader, StaticFileSegment,
};
use reth_provider::{
    providers::StaticFileProvider, BlockWriter, StageCheckpointWriter, StaticFileWriter,
};
use reth_stages::{StageCheckpoint, StageId};
use tracing::info;

/// Creates a dummy chain (with no transactions) up to the last OVM block and appends the
/// first valid Bedrock block.
pub(crate) fn setup_op_mainnet_without_ovm<Provider>(
    provider_rw: &Provider,
    static_file_provider: &StaticFileProvider,
) -> Result<(), eyre::Error>
where
    Provider: StageCheckpointWriter + BlockWriter,
{
    info!(target: "reth::cli", "Setting up dummy OVM chain before importing state.");

    // Write OVM dummy data up to `BEDROCK_HEADER - 1` block
    append_dummy_chain(static_file_provider, BEDROCK_HEADER.number - 1)?;

    info!(target: "reth::cli", "Appending Bedrock block.");

    append_bedrock_block(provider_rw, static_file_provider)?;

    for stage in StageId::ALL {
        provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(BEDROCK_HEADER.number))?;
    }

    info!(target: "reth::cli", "Set up finished.");

    Ok(())
}

/// Appends the first bedrock block.
///
/// By appending it, static file writer also verifies that all segments are at the same
/// height.
fn append_bedrock_block(
    provider_rw: impl BlockWriter,
    sf_provider: &StaticFileProvider,
) -> Result<(), eyre::Error> {
    provider_rw.insert_block(
        SealedBlockWithSenders::new(
            SealedBlock::new(
                SealedHeader::new(BEDROCK_HEADER, BEDROCK_HEADER_HASH),
                BlockBody::default(),
            ),
            vec![],
        )
        .expect("no senders or txes"),
    )?;

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

/// Creates a dummy chain with no transactions/receipts up to `target_height` block inclusive.
///
/// * Headers: It will push an empty block.
/// * Transactions: It will not push any tx, only increments the end block range.
/// * Receipts: It will not push any receipt, only increments the end block range.
fn append_dummy_chain(
    sf_provider: &StaticFileProvider,
    target_height: BlockNumber,
) -> Result<(), eyre::Error> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Spawn jobs for incrementing the block end range of transactions and receipts
    for segment in [StaticFileSegment::Transactions, StaticFileSegment::Receipts] {
        let tx_clone = tx.clone();
        let provider = sf_provider.clone();
        std::thread::spawn(move || {
            let result = provider.latest_writer(segment).and_then(|mut writer| {
                for block_num in 1..=target_height {
                    writer.increment_block(block_num)?;
                }
                Ok(())
            });

            tx_clone.send(result).unwrap();
        });
    }

    // Spawn job for appending empty headers
    let provider = sf_provider.clone();
    std::thread::spawn(move || {
        let mut empty_header = Header::default();
        let result = provider.latest_writer(StaticFileSegment::Headers).and_then(|mut writer| {
            for block_num in 1..=target_height {
                // TODO: should we fill with real parent_hash?
                empty_header.number = block_num;
                writer.append_header(&empty_header, U256::ZERO, &B256::ZERO)?;
            }
            Ok(())
        });

        tx.send(result).unwrap();
    });

    // Catches any StaticFileWriter error.
    while let Ok(r) = rx.recv() {
        r?;
    }

    // If, for any reason, rayon crashes this verifies if all segments are at the same
    // target_height.
    for segment in
        [StaticFileSegment::Headers, StaticFileSegment::Receipts, StaticFileSegment::Transactions]
    {
        assert_eq!(
            sf_provider.latest_writer(segment)?.user_header().block_end(),
            Some(target_height),
            "Static file segment {segment} was unsuccessful advancing its block height."
        );
    }

    Ok(())
}
