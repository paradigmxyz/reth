//! Command that imports OP mainnet receipts from Bedrock datadir, exported via
//! <https://github.com/testinprod-io/op-geth/pull/1>.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Parser;
use reth_db::{database::Database, init_db, tables, transaction::DbTx};
use reth_downloaders::{
    file_client::{ChunkedFileReader, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE},
    receipt_file_client::ReceiptFileClient,
};
use reth_node_core::version::SHORT_VERSION;
use reth_optimism_primitives::bedrock_import::is_dup_tx;
use reth_primitives::{stage::StageId, Receipts, StaticFileSegment};
use reth_provider::{
    providers::StaticFileProvider, BundleStateWithReceipts, OriginalValuesKnown, ProviderFactory,
    StageCheckpointReader, StateWriter, StaticFileProviderFactory, StaticFileWriter, StatsReader,
};
use tracing::{debug, error, info, trace};

use crate::{
    args::{
        utils::{genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct ImportReceiptsOpCommand {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// Chunk byte length to read from file.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    #[command(flatten)]
    db: DatabaseArgs,

    /// The path to a receipts file for import. File must use `HackReceiptFileCodec` (used for
    /// exporting OP chain segment below Bedrock block via testinprod/op-geth).
    ///
    /// <https://github.com/testinprod-io/op-geth/pull/1>
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl ImportReceiptsOpCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
            "Chunking receipts import"
        );

        let chain_spec = genesis_value_parser(SUPPORTED_CHAINS[0])?;

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(chain_spec.chain);

        let db_path = data_dir.db();
        info!(target: "reth::cli", path = ?db_path, "Opening database");

        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");
        let provider_factory = ProviderFactory::new(
            db.clone(),
            chain_spec.clone(),
            StaticFileProvider::read_write(data_dir.static_files())?,
        );

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

/// Imports receipts to static files. Takes a filter callback as parameter, that returns the total
/// number of filtered out receipts.
///
/// Caution! Filter callback must replace completely filtered out receipts for a block, with empty
/// vectors, rather than `vec!(None)`. This is since the code for writing to static files, expects
/// indices in the [`Receipts`] list, to map to sequential block numbers.
pub async fn import_receipts_from_file<DB, P, F>(
    provider_factory: ProviderFactory<DB>,
    path: P,
    chunk_len: Option<u64>,
    mut filter: F,
) -> eyre::Result<()>
where
    DB: Database,
    P: AsRef<Path>,
    F: FnMut(u64, &mut Receipts) -> usize,
{
    let provider = provider_factory.provider_rw()?;
    let static_file_provider = provider_factory.static_file_provider();

    let total_imported_txns = static_file_provider
        .count_entries::<tables::Transactions>()
        .expect("transaction static files must exist before importing receipts");
    let highest_block_transactions = static_file_provider
        .get_highest_static_file_block(StaticFileSegment::Transactions)
        .expect("transaction static files must exist before importing receipts");

    for stage in StageId::ALL {
        let checkpoint = provider.get_stage_checkpoint(stage)?;
        trace!(target: "reth::cli",
            ?stage,
            ?checkpoint,
            "Read stage checkpoints from db"
        );
    }

    // prepare the tx for `write_to_storage`
    let tx = provider.into_tx();
    let mut total_decoded_receipts = 0;
    let mut total_filtered_out_dup_txns = 0;

    // open file
    let mut reader = ChunkedFileReader::new(path, chunk_len).await?;

    while let Some(file_client) = reader.next_chunk::<ReceiptFileClient>().await? {
        // create a new file client from chunk read from file
        let ReceiptFileClient { mut receipts, first_block, total_receipts: total_receipts_chunk } =
            file_client;

        // mark these as decoded
        total_decoded_receipts += total_receipts_chunk;

        total_filtered_out_dup_txns += filter(first_block, &mut receipts);

        info!(target: "reth::cli",
            first_receipts_block=?first_block,
            total_receipts_chunk,
            "Importing receipt file chunk"
        );

        // We're reusing receipt writing code internal to
        // `BundleStateWithReceipts::write_to_storage`, so we just use a default empty
        // `BundleState`.
        let bundled_state = BundleStateWithReceipts::new(Default::default(), receipts, first_block);

        let static_file_producer =
            static_file_provider.get_writer(first_block, StaticFileSegment::Receipts)?;

        // finally, write the receipts
        bundled_state.write_to_storage::<DB::TXMut>(
            &tx,
            Some(static_file_producer),
            OriginalValuesKnown::Yes,
        )?;
    }

    tx.commit()?;
    // as static files works in file ranges, internally it will be committing when creating the
    // next file range already, so we only need to call explicitly at the end.
    static_file_provider.commit()?;

    if total_decoded_receipts == 0 {
        error!(target: "reth::cli", "No receipts were imported, ensure the receipt file is valid and not empty");
        return Ok(())
    }

    let total_imported_receipts = static_file_provider
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

    let highest_block_receipts = static_file_provider
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
