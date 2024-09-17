//! Command that imports OP mainnet receipts from Bedrock datadir, exported via
//! <https://github.com/testinprod-io/op-geth/pull/1>.

use std::path::{Path, PathBuf};

use clap::Parser;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_db::tables;
use reth_downloaders::{
    file_client::{ChunkedFileReader, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE},
    receipt_file_client::ReceiptFileClient,
};
use reth_execution_types::ExecutionOutcome;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithEngine};
use reth_node_core::version::SHORT_VERSION;
use reth_optimism_primitives::bedrock::is_dup_tx;
use reth_primitives::Receipts;
use reth_provider::{
    writer::UnifiedStorageWriter, DatabaseProviderFactory, OriginalValuesKnown, ProviderFactory,
    StageCheckpointReader, StateWriter, StaticFileProviderFactory, StaticFileWriter, StatsReader,
};
use reth_stages::StageId;
use reth_static_file_types::StaticFileSegment;
use tracing::{debug, error, info, trace};

use crate::receipt_file_codec::HackReceiptFileCodec;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct ImportReceiptsOpCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Chunk byte length to read from file.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    /// The path to a receipts file for import. File must use `HackReceiptFileCodec` (used for
    /// exporting OP chain segment below Bedrock block via testinprod/op-geth).
    ///
    /// <https://github.com/testinprod-io/op-geth/pull/1>
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl<C: ChainSpecParser<ChainSpec = ChainSpec>> ImportReceiptsOpCommand<C> {
    /// Execute `import` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
            "Chunking receipts import"
        );

        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

        import_receipts_from_file(
            provider_factory,
            self.path,
            self.chunk_len,
            |first_block, receipts: &mut Receipts| {
                let mut total_filtered_out_dup_txns = 0;
                for (index, receipts_for_block) in receipts.iter_mut().enumerate() {
                    if is_dup_tx(first_block + index as u64) {
                        receipts_for_block.clear();
                        total_filtered_out_dup_txns += 1;
                    }
                }

                total_filtered_out_dup_txns
            },
        )
        .await
    }
}

/// Imports receipts to static files from file in chunks. See [`import_receipts_from_reader`].
pub async fn import_receipts_from_file<N, P, F>(
    provider_factory: ProviderFactory<N>,
    path: P,
    chunk_len: Option<u64>,
    filter: F,
) -> eyre::Result<()>
where
    N: NodeTypesWithDB<ChainSpec = ChainSpec>,
    P: AsRef<Path>,
    F: FnMut(u64, &mut Receipts) -> usize,
{
    let total_imported_txns = provider_factory
        .static_file_provider()
        .count_entries::<tables::Transactions>()
        .expect("transaction static files must exist before importing receipts");
    let highest_block_transactions = provider_factory
        .static_file_provider()
        .get_highest_static_file_block(StaticFileSegment::Transactions)
        .expect("transaction static files must exist before importing receipts");

    for stage in StageId::ALL {
        let checkpoint = provider_factory.database_provider_ro()?.get_stage_checkpoint(stage)?;
        trace!(target: "reth::cli",
            ?stage,
            ?checkpoint,
            "Read stage checkpoints from db"
        );
    }

    // open file
    let reader = ChunkedFileReader::new(&path, chunk_len).await?;

    // import receipts
    let ImportReceiptsResult { total_decoded_receipts, total_filtered_out_dup_txns } =
        import_receipts_from_reader(&provider_factory, reader, filter).await?;

    if total_decoded_receipts == 0 {
        error!(target: "reth::cli", "No receipts were imported, ensure the receipt file is valid and not empty");
        return Ok(())
    }

    let total_imported_receipts = provider_factory
        .static_file_provider()
        .count_entries::<tables::Receipts>()
        .expect("static files must exist after ensuring we decoded more than zero");

    if total_imported_receipts + total_filtered_out_dup_txns != total_decoded_receipts {
        error!(target: "reth::cli",
            total_decoded_receipts,
            total_imported_receipts,
            total_filtered_out_dup_txns,
            "Receipts were partially imported"
        );
    }

    if total_imported_receipts != total_imported_txns {
        error!(target: "reth::cli",
            total_imported_receipts,
            total_imported_txns,
            "Receipts inconsistent with transactions"
        );
    }

    let highest_block_receipts = provider_factory
        .static_file_provider()
        .get_highest_static_file_block(StaticFileSegment::Receipts)
        .expect("static files must exist after ensuring we decoded more than zero");

    if highest_block_receipts != highest_block_transactions {
        error!(target: "reth::cli",
            highest_block_receipts,
            highest_block_transactions,
            "Height of receipts inconsistent with transactions"
        );
    }

    info!(target: "reth::cli",
        total_imported_receipts,
        total_decoded_receipts,
        total_filtered_out_dup_txns,
        "Receipt file imported"
    );

    Ok(())
}

/// Imports receipts to static files. Takes a filter callback as parameter, that returns the total
/// number of filtered out receipts.
///
/// Caution! Filter callback must replace completely filtered out receipts for a block, with empty
/// vectors, rather than `vec!(None)`. This is since the code for writing to static files, expects
/// indices in the [`Receipts`] list, to map to sequential block numbers.
pub async fn import_receipts_from_reader<N, F>(
    provider_factory: &ProviderFactory<N>,
    mut reader: ChunkedFileReader,
    mut filter: F,
) -> eyre::Result<ImportReceiptsResult>
where
    N: NodeTypesWithDB<ChainSpec = ChainSpec>,
    F: FnMut(u64, &mut Receipts) -> usize,
{
    let mut total_decoded_receipts = 0;
    let mut total_filtered_out_dup_txns = 0;

    let provider = provider_factory.provider_rw()?;
    let static_file_provider = provider_factory.static_file_provider();

    while let Some(file_client) =
        reader.next_receipts_chunk::<ReceiptFileClient<_>, HackReceiptFileCodec>().await?
    {
        // create a new file client from chunk read from file
        let ReceiptFileClient {
            mut receipts,
            mut first_block,
            total_receipts: total_receipts_chunk,
            ..
        } = file_client;

        // mark these as decoded
        total_decoded_receipts += total_receipts_chunk;

        total_filtered_out_dup_txns += filter(first_block, &mut receipts);

        info!(target: "reth::cli",
            first_receipts_block=?first_block,
            total_receipts_chunk,
            "Importing receipt file chunk"
        );

        // It is possible for the first receipt returned by the file client to be the genesis
        // block. In this case, we just prepend empty receipts to the current list of receipts.
        // When initially writing to static files, the provider expects the first block to be block
        // one. So, if the first block returned by the file client is the genesis block, we remove
        // those receipts.
        if first_block == 0 {
            // remove the first empty receipts
            let genesis_receipts = receipts.remove(0);
            debug_assert!(genesis_receipts.is_empty());
            // this ensures the execution outcome and static file producer start at block 1
            first_block = 1;
        }

        // We're reusing receipt writing code internal to
        // `UnifiedStorageWriter::append_receipts_from_blocks`, so we just use a default empty
        // `BundleState`.
        let execution_outcome =
            ExecutionOutcome::new(Default::default(), receipts, first_block, Default::default());

        let static_file_producer =
            static_file_provider.get_writer(first_block, StaticFileSegment::Receipts)?;

        // finally, write the receipts
        let mut storage_writer = UnifiedStorageWriter::from(&provider, static_file_producer);
        storage_writer.write_to_storage(execution_outcome, OriginalValuesKnown::Yes)?;
    }

    // as static files works in file ranges, internally it will be committing when creating the
    // next file range already, so we only need to call explicitly at the end.
    UnifiedStorageWriter::commit(provider, static_file_provider)?;

    Ok(ImportReceiptsResult { total_decoded_receipts, total_filtered_out_dup_txns })
}

/// Result of importing receipts in chunks.
#[derive(Debug)]
pub struct ImportReceiptsResult {
    total_decoded_receipts: usize,
    total_filtered_out_dup_txns: usize,
}

#[cfg(test)]
mod test {
    use alloy_primitives::hex;
    use reth_db_common::init::init_genesis;
    use reth_stages::test_utils::TestStageDB;
    use tempfile::tempfile;
    use tokio::{
        fs::File,
        io::{AsyncSeekExt, AsyncWriteExt, SeekFrom},
    };

    use crate::receipt_file_codec::test::{
        HACK_RECEIPT_ENCODED_BLOCK_1, HACK_RECEIPT_ENCODED_BLOCK_2, HACK_RECEIPT_ENCODED_BLOCK_3,
    };

    use super::*;

    /// No receipts for genesis block
    const EMPTY_RECEIPTS_GENESIS_BLOCK: &[u8] = &hex!("c0");

    #[ignore]
    #[tokio::test]
    async fn filter_out_genesis_block_receipts() {
        let mut f: File = tempfile().unwrap().into();
        f.write_all(EMPTY_RECEIPTS_GENESIS_BLOCK).await.unwrap();
        f.write_all(HACK_RECEIPT_ENCODED_BLOCK_1).await.unwrap();
        f.write_all(HACK_RECEIPT_ENCODED_BLOCK_2).await.unwrap();
        f.write_all(HACK_RECEIPT_ENCODED_BLOCK_3).await.unwrap();
        f.flush().await.unwrap();
        f.seek(SeekFrom::Start(0)).await.unwrap();

        let reader =
            ChunkedFileReader::from_file(f, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE).await.unwrap();

        let db = TestStageDB::default();
        init_genesis(&db.factory).unwrap();

        // todo: where does import command init receipts ? probably somewhere in pipeline

        let ImportReceiptsResult { total_decoded_receipts, total_filtered_out_dup_txns } =
            import_receipts_from_reader(&TestStageDB::default().factory, reader, |_, _| 0)
                .await
                .unwrap();

        assert_eq!(total_decoded_receipts, 3);
        assert_eq!(total_filtered_out_dup_txns, 0);
    }
}
