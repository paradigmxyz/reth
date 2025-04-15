use futures_util::StreamExt;
use reth_db_api::transaction::DbTxMut;
use reth_era_downloader::{EraStream, HttpClient};
use reth_provider::{
    ProviderError, StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
};
use reth_storage_api::{DBProvider, HeaderProvider};

pub async fn import<P>(mut downloader: EraStream<impl HttpClient>, provider: &P) -> eyre::Result<()>
where
    P: DBProvider<Tx: DbTxMut> + StaticFileProviderFactory,
{
    let static_file_provider = provider.static_file_provider();

    // Consistency check of expected headers in static files vs DB is done on provider::sync_gap
    // when poll_execute_ready is polled.
    let mut last_header_number = static_file_provider
        .get_highest_static_file_block(StaticFileSegment::Headers)
        .unwrap_or_default();

    // Find the latest total difficulty
    let mut td = static_file_provider
        .header_td_by_number(last_header_number)?
        .ok_or(ProviderError::TotalDifficultyNotFound(last_header_number))?;

    // Although headers were downloaded in reverse order, the collector iterates it in ascending
    // order
    let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;

    while let Some(file) = downloader.next().await {
        let file = file?;

        // reth_era::era1_file::Era1Reader::new()

        // writer.append_header(&header, td, &header_hash)?;
    }

    Ok(())
}
