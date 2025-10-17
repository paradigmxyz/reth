use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockNumber, B256, U256};
use alloy_rlp::Decodable;
use reth_codecs::Compact;
use reth_node_builder::NodePrimitives;
use reth_primitives_traits::{SealedBlock, SealedHeader, SealedHeaderFor};
use reth_provider::{
    providers::StaticFileProvider, BlockWriter, ProviderResult, StageCheckpointWriter,
    StaticFileProviderFactory, StaticFileWriter,
};
use reth_stages::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use std::path::Path;
use tracing::info;

/// Reads the header RLP from a file and returns the Header.
///
/// This supports both raw rlp bytes and rlp hex string.
pub(crate) fn read_header_from_file<H>(path: &Path) -> Result<H, eyre::Error>
where
    H: Decodable,
{
    let buf = if let Ok(content) = reth_fs_util::read_to_string(path) {
        alloy_primitives::hex::decode(content.trim())?
    } else {
        // If UTF-8 decoding fails, read as raw bytes
        reth_fs_util::read(path)?
    };

    let header = H::decode(&mut &buf[..])?;
    Ok(header)
}

/// Creates a dummy chain (with no transactions) up to the last EVM block and appends the
/// first valid block.
pub fn setup_without_evm<Provider, F>(
    provider_rw: &Provider,
    header: SealedHeader<<Provider::Primitives as NodePrimitives>::BlockHeader>,
    header_factory: F,
) -> ProviderResult<()>
where
    Provider: StaticFileProviderFactory
        + StageCheckpointWriter
        + BlockWriter<Block = <Provider::Primitives as NodePrimitives>::Block>,
    F: Fn(BlockNumber) -> <Provider::Primitives as NodePrimitives>::BlockHeader
        + Send
        + Sync
        + 'static,
{
    info!(target: "reth::cli", new_tip = ?header.num_hash(), "Setting up dummy EVM chain before importing state.");

    let static_file_provider = provider_rw.static_file_provider();
    // Write EVM dummy data up to `header - 1` block
    append_dummy_chain(&static_file_provider, header.number() - 1, header_factory)?;

    info!(target: "reth::cli", "Appending first valid block.");

    append_first_block(provider_rw, &header)?;

    for stage in StageId::ALL {
        provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(header.number()))?;
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
    header: &SealedHeaderFor<Provider::Primitives>,
) -> ProviderResult<()>
where
    Provider: BlockWriter<Block = <Provider::Primitives as NodePrimitives>::Block>
        + StaticFileProviderFactory<Primitives: NodePrimitives<BlockHeader: Compact>>,
{
    provider_rw.insert_block(
        SealedBlock::<<Provider::Primitives as NodePrimitives>::Block>::from_sealed_parts(
            header.clone(),
            Default::default(),
        )
        .try_recover()
        .expect("no senders or txes"),
    )?;

    let sf_provider = provider_rw.static_file_provider();

    sf_provider.latest_writer(StaticFileSegment::Receipts)?.increment_block(header.number())?;

    Ok(())
}

/// Creates a dummy chain with no transactions/receipts up to `target_height` block inclusive.
///
/// * Headers: It will push an empty block.
/// * Transactions: It will not push any tx, only increments the end block range.
/// * Receipts: It will not push any receipt, only increments the end block range.
fn append_dummy_chain<N, F>(
    sf_provider: &StaticFileProvider<N>,
    target_height: BlockNumber,
    header_factory: F,
) -> ProviderResult<()>
where
    N: NodePrimitives,
    F: Fn(BlockNumber) -> N::BlockHeader + Send + Sync + 'static,
{
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
        let result = provider.latest_writer(StaticFileSegment::Headers).and_then(|mut writer| {
            for block_num in 1..=target_height {
                // TODO: should we fill with real parent_hash?
                let header = header_factory(block_num);
                writer.append_header(&header, U256::ZERO, &B256::ZERO)?;
            }
            Ok(())
        });

        tx.send(result).unwrap();
    });

    // Catches any StaticFileWriter error.
    while let Ok(append_result) = rx.recv() {
        if let Err(err) = append_result {
            tracing::error!(target: "reth::cli", "Error appending dummy chain: {err}");
            return Err(err)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{address, b256};
    use reth_db_common::init::init_genesis;
    use reth_provider::{test_utils::create_test_provider_factory, DatabaseProviderFactory};
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_header_from_file_hex_string() {
        let header_rlp = "0xf90212a00d84d79f59fc384a1f6402609a5b7253b4bfe7a4ae12608ed107273e5422b6dda01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d493479471562b71999873db5b286df957af199ec94617f7a0f496f3d199c51a1aaee67dac95f24d92ac13c60d25181e1eecd6eca5ddf32ac0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000808206a4840365908a808468e975f09ad983011003846765746888676f312e32352e308664617277696ea06f485a167165ec12e0ab3e6ab59a7b88560b90306ac98a26eb294abf95a8c59b88000000000000000007";

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(header_rlp.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let header: Header = read_header_from_file(temp_file.path()).unwrap();

        assert_eq!(header.number, 1700);
        assert_eq!(
            header.parent_hash,
            b256!("0d84d79f59fc384a1f6402609a5b7253b4bfe7a4ae12608ed107273e5422b6dd")
        );
        assert_eq!(header.beneficiary, address!("71562b71999873db5b286df957af199ec94617f7"));
    }

    #[test]
    fn test_read_header_from_file_raw_bytes() {
        let header_rlp = "0xf90212a00d84d79f59fc384a1f6402609a5b7253b4bfe7a4ae12608ed107273e5422b6dda01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d493479471562b71999873db5b286df957af199ec94617f7a0f496f3d199c51a1aaee67dac95f24d92ac13c60d25181e1eecd6eca5ddf32ac0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000808206a4840365908a808468e975f09ad983011003846765746888676f312e32352e308664617277696ea06f485a167165ec12e0ab3e6ab59a7b88560b90306ac98a26eb294abf95a8c59b88000000000000000007";
        let header_bytes =
            alloy_primitives::hex::decode(header_rlp.trim_start_matches("0x")).unwrap();

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&header_bytes).unwrap();
        temp_file.flush().unwrap();

        let header: Header = read_header_from_file(temp_file.path()).unwrap();

        assert_eq!(header.number, 1700);
        assert_eq!(
            header.parent_hash,
            b256!("0d84d79f59fc384a1f6402609a5b7253b4bfe7a4ae12608ed107273e5422b6dd")
        );
        assert_eq!(header.beneficiary, address!("71562b71999873db5b286df957af199ec94617f7"));
    }

    #[test]
    fn test_setup_without_evm_succeeds() {
        let header_rlp = "0xf90212a00d84d79f59fc384a1f6402609a5b7253b4bfe7a4ae12608ed107273e5422b6dda01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d493479471562b71999873db5b286df957af199ec94617f7a0f496f3d199c51a1aaee67dac95f24d92ac13c60d25181e1eecd6eca5ddf32ac0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000808206a4840365908a808468e975f09ad983011003846765746888676f312e32352e308664617277696ea06f485a167165ec12e0ab3e6ab59a7b88560b90306ac98a26eb294abf95a8c59b88000000000000000007";
        let header_bytes =
            alloy_primitives::hex::decode(header_rlp.trim_start_matches("0x")).unwrap();

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&header_bytes).unwrap();
        temp_file.flush().unwrap();

        let header: Header = read_header_from_file(temp_file.path()).unwrap();
        let header_hash = b256!("4f05e4392969fc82e41f6d6a8cea379323b0b2d3ddf7def1a33eec03883e3a33");

        let provider_factory = create_test_provider_factory();

        init_genesis(&provider_factory).unwrap();

        let provider_rw = provider_factory.database_provider_rw().unwrap();

        setup_without_evm(&provider_rw, SealedHeader::new(header, header_hash), |number| Header {
            number,
            ..Default::default()
        })
        .unwrap();

        let static_files = provider_factory.static_file_provider();
        let writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        let actual_next_height = writer.next_block_number();
        let expected_next_height = 1701;

        assert_eq!(actual_next_height, expected_next_height);
    }
}
