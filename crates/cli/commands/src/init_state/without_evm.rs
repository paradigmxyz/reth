use alloy_primitives::{BlockNumber, B256, U256};
use alloy_rlp::Decodable;

use alloy_consensus::Header;
use reth_node_builder::NodePrimitives;
use reth_primitives::{SealedBlock, SealedBlockWithSenders, SealedHeader, StaticFileSegment};
use reth_provider::{
    providers::StaticFileProvider, BlockWriter, StageCheckpointWriter, StaticFileProviderFactory,
    StaticFileWriter, StorageLocation,
};
use reth_stages::{StageCheckpoint, StageId};

use std::{fs::File, io::Read, path::PathBuf};
use tracing::info;

/// Reads the header RLP from a file and returns the Header.
pub(crate) fn read_header_from_file(path: PathBuf) -> Result<Header, eyre::Error> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let header = Header::decode(&mut &buf[..])?;
    Ok(header)
}

/// Creates a dummy chain (with no transactions) up to the last EVM block and appends the
/// first valid block.
pub fn setup_without_evm<Provider>(
    provider_rw: &Provider,
    header: SealedHeader,
    total_difficulty: U256,
) -> Result<(), eyre::Error>
where
    Provider: StaticFileProviderFactory
        + StageCheckpointWriter
        + BlockWriter<Block: reth_node_api::Block<Header = reth_primitives::Header>>,
{
    info!(target: "reth::cli", "Setting up dummy EVM chain before importing state.");

    let static_file_provider = provider_rw.static_file_provider();
    // Write EVM dummy data up to `header - 1` block
    append_dummy_chain(&static_file_provider, header.number - 1)?;

    info!(target: "reth::cli", "Appending first valid block.");

    append_first_block(provider_rw, &header, total_difficulty)?;

    for stage in StageId::ALL {
        provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(header.number))?;
    }

    info!(target: "reth::cli", "Set up finished.");

    Ok(())
}

/// Appends the first block.
///
/// By appending it, static file writer also verifies that all segments are at the same
/// height.
fn append_first_block<Provider>(
    provider_rw: &Provider,
    header: &SealedHeader,
    total_difficulty: U256,
) -> Result<(), eyre::Error>
where
    Provider: BlockWriter<Block: reth_node_api::Block<Header = reth_primitives::Header>>
        + StaticFileProviderFactory,
{
    provider_rw.insert_block(
        SealedBlockWithSenders::new(SealedBlock::new(header.clone(), Default::default()), vec![])
            .expect("no senders or txes"),
        StorageLocation::Database,
    )?;

    let sf_provider = provider_rw.static_file_provider();

    sf_provider.latest_writer(StaticFileSegment::Headers)?.append_header(
        header,
        total_difficulty,
        &header.hash(),
    )?;

    sf_provider.latest_writer(StaticFileSegment::Receipts)?.increment_block(header.number)?;

    sf_provider.latest_writer(StaticFileSegment::Transactions)?.increment_block(header.number)?;

    Ok(())
}

/// Creates a dummy chain with no transactions/receipts up to `target_height` block inclusive.
///
/// * Headers: It will push an empty block.
/// * Transactions: It will not push any tx, only increments the end block range.
/// * Receipts: It will not push any receipt, only increments the end block range.
fn append_dummy_chain<N: NodePrimitives>(
    sf_provider: &StaticFileProvider<N>,
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
