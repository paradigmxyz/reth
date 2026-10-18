use crate::{client::HttpClient, EraClient, BLOCKS_PER_FILE};
use alloy_primitives::BlockNumber;
use futures_util::{stream::FuturesOrdered, FutureExt, Stream, StreamExt};
use reqwest::Url;
use reth_fs_util as fs;
use std::{
    collections::VecDeque,
    fmt::{Debug, Formatter},
    future::Future,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};

/// Parameters that alter the behavior of [`EraStream`].
///
/// # Examples
/// ```
/// use reth_era_downloader::EraStreamConfig;
///
/// EraStreamConfig::default().with_max_files(10).with_max_concurrent_downloads(2);
/// ```
#[derive(Debug, Clone)]
pub struct EraStreamConfig {
    max_files: usize,
    max_concurrent_downloads: usize,
    start_from: Option<usize>,
}

impl Default for EraStreamConfig {
    fn default() -> Self {
        Self { max_files: 5, max_concurrent_downloads: 3, start_from: None }
    }
}

impl EraStreamConfig {
    /// The maximum number of downloaded archives kept in the dedicated cache directory.
    ///
    /// # Panics
    ///
    /// Panics if `max_files` is zero: at least one file must fit for the stream to make progress.
    #[track_caller]
    pub const fn with_max_files(mut self, max_files: usize) -> Self {
        assert!(max_files > 0, "maximum cached files must be nonzero");
        self.max_files = max_files;
        self
    }

    /// The maximum number of downloads happening at the same time.
    ///
    /// # Panics
    ///
    /// Panics if `max_concurrent_downloads` is zero: downloads would never be polled.
    #[track_caller]
    pub const fn with_max_concurrent_downloads(mut self, max_concurrent_downloads: usize) -> Self {
        assert!(max_concurrent_downloads > 0, "maximum concurrent downloads must be nonzero");
        self.max_concurrent_downloads = max_concurrent_downloads;
        self
    }

    /// Overrides the starting ERA file index to be the first one that contains `block_number`.
    pub const fn start_from(mut self, block_number: BlockNumber) -> Self {
        self.start_from.replace(block_number as usize / BLOCKS_PER_FILE);
        self
    }
}

/// An asynchronous stream of downloaded archives backed by a dedicated, disposable cache.
///
/// On startup, archives of the client's type outside the initial configured window are deleted,
/// including preexisting files that have not been processed. Do not point this stream at an archive
/// directory whose contents must be preserved.
///
/// Each yielded [`EraRemoteMeta`] keeps its file in the cache until
/// [`EraMeta::mark_as_processed`] removes it. Close file handles before acknowledging a file. If
/// files are retained without acknowledgement, the cache fills and further downloads wait for
/// space. Dropping metadata or the stream does not acknowledge files; retained files can be reused
/// on the next run if they fall inside that run's initial window.
///
/// # Examples
/// ```
/// use futures_util::StreamExt;
/// use reth_era_downloader::{EraMeta, EraStream, HttpClient};
///
/// # async fn import(mut stream: EraStream<impl HttpClient + Clone + Send + Sync + 'static + Unpin>) -> eyre::Result<()> {
/// while let Some(meta) = stream.next().await {
///     let meta = meta?;
///     // Process file at `meta.path(): &Path`
///     meta.mark_as_processed()?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct EraStream<Http> {
    download_stream: DownloadStream,
    starting_stream: StartingStream<Http>,
}

impl<Http> EraStream<Http> {
    /// Constructs a stream that downloads into the client's dedicated, disposable cache directory.
    ///
    /// Before downloading, the first poll fetches the file list and deletes matching archives
    /// outside `[start_from, start_from + max_files)`. This includes existing unprocessed archives;
    /// unrelated files and archives of other types are retained. Keep archival data elsewhere.
    ///
    /// Download concurrency and retained file count are bounded by `config`. After processing each
    /// yielded file, call [`EraMeta::mark_as_processed`] to remove it and free cache capacity. The
    /// cache directory must already exist; access errors are yielded as stream errors.
    pub fn new(client: EraClient<Http>, config: EraStreamConfig) -> Self {
        Self {
            download_stream: DownloadStream {
                downloads: Default::default(),
                scheduled: Default::default(),
                max_concurrent_downloads: config.max_concurrent_downloads,
                ended: false,
            },
            starting_stream: StartingStream {
                client,
                files_count: Box::pin(async move { Ok(usize::MAX) }),
                next_url: Box::pin(async move { Ok(None) }),
                delete_outside_range: Box::pin(async move { Ok(()) }),
                recover_index: Box::pin(async move { Ok(None) }),
                fetch_file_list: Box::pin(async move { Ok(()) }),
                state: Default::default(),
                max_files: config.max_files,
                index: config.start_from.unwrap_or_default(),
                last: None,
                downloading: 0,
            },
        }
    }
}

/// Contains information about an ERA file.
pub trait EraMeta: Debug {
    /// Marking this particular ERA file as "processed" lets the caller hint that it is no longer
    /// going to be using it.
    ///
    /// The meaning of that is up to the implementation. The caller should assume that after this
    /// point is no longer possible to safely read it.
    fn mark_as_processed(&self) -> eyre::Result<()>;

    /// A path to the era file.
    ///
    /// File should be openable and treated as read-only.
    fn path(&self) -> &Path;
}

impl<T: EraMeta> EraMeta for Box<T> {
    fn mark_as_processed(&self) -> eyre::Result<()> {
        T::mark_as_processed(self)
    }

    fn path(&self) -> &Path {
        T::path(self)
    }
}

/// A remotely hosted archive represented by a downloaded file in a disposable cache.
///
/// Process the file at [`EraMeta::path`], close its handles, and then call
/// [`EraMeta::mark_as_processed`] to delete it and release cache capacity. Dropping this metadata
/// does not delete the file.
#[derive(Debug)]
pub struct EraRemoteMeta {
    path: Box<Path>,
}

impl EraRemoteMeta {
    const fn new(path: Box<Path>) -> Self {
        Self { path }
    }
}

impl AsRef<Path> for EraRemoteMeta {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

impl EraMeta for EraRemoteMeta {
    /// Removes a temporary local file representation of the remotely hosted original.
    fn mark_as_processed(&self) -> eyre::Result<()> {
        Ok(fs::remove_file(&self.path)?)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl<Http: HttpClient + Clone + Send + Sync + 'static + Unpin> Stream for EraStream<Http> {
    type Item = eyre::Result<EraRemoteMeta>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Poll::Ready(fut) = self.starting_stream.poll_next_unpin(cx) {
            match fut {
                Some(Ok(fut)) => self.download_stream.scheduled.push_back(fut),
                Some(Err(error)) => return Poll::Ready(Some(Err(error))),
                None => self.download_stream.ended = true,
            }
        }

        let poll = self.download_stream.poll_next_unpin(cx);

        if poll.is_ready() {
            self.starting_stream.downloaded();
        }

        poll
    }
}

type DownloadFuture =
    Pin<Box<dyn Future<Output = eyre::Result<EraRemoteMeta>> + Send + Sync + 'static>>;

struct DownloadStream {
    downloads: FuturesOrdered<DownloadFuture>,
    scheduled: VecDeque<DownloadFuture>,
    max_concurrent_downloads: usize,
    ended: bool,
}

impl Debug for DownloadStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DownloadStream({})", self.downloads.len())
    }
}

impl Stream for DownloadStream {
    type Item = eyre::Result<EraRemoteMeta>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        for _ in 0..self.max_concurrent_downloads - self.downloads.len() {
            if let Some(fut) = self.scheduled.pop_front() {
                self.downloads.push_back(fut);
            }
        }

        let ended = self.ended;
        let poll = self.downloads.poll_next_unpin(cx);

        if matches!(poll, Poll::Ready(None)) && !ended {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        poll
    }
}

struct StartingStream<Http> {
    client: EraClient<Http>,
    files_count: Pin<Box<dyn Future<Output = eyre::Result<usize>> + Send + Sync + 'static>>,
    next_url: Pin<Box<dyn Future<Output = eyre::Result<Option<Url>>> + Send + Sync + 'static>>,
    delete_outside_range: Pin<Box<dyn Future<Output = eyre::Result<()>> + Send + Sync + 'static>>,
    recover_index:
        Pin<Box<dyn Future<Output = eyre::Result<Option<usize>>> + Send + Sync + 'static>>,
    fetch_file_list: Pin<Box<dyn Future<Output = eyre::Result<()>> + Send + Sync + 'static>>,
    state: State,
    max_files: usize,
    index: usize,
    last: Option<usize>,
    downloading: usize,
}

impl<Http> Debug for StartingStream<Http> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StartingStream{{ max_files: {}, index: {}, downloading: {} }}",
            self.max_files, self.index, self.downloading
        )
    }
}

#[derive(Debug, PartialEq, Default)]
enum State {
    #[default]
    Initial,
    FetchFileList,
    DeleteOutsideRange,
    RecoverIndex,
    CountFiles,
    Missing(usize),
    NextUrl(usize),
}

impl<Http: HttpClient + Clone + Send + Sync + 'static + Unpin> Stream for StartingStream<Http> {
    type Item = eyre::Result<DownloadFuture>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.state == State::Initial {
            self.fetch_file_list();
        }

        if self.state == State::FetchFileList &&
            let Poll::Ready(result) = self.fetch_file_list.poll_unpin(cx)
        {
            match result {
                Ok(_) => self.delete_outside_range(),
                Err(e) => {
                    self.fetch_file_list();

                    return Poll::Ready(Some(Err(e)));
                }
            }
        }

        if self.state == State::DeleteOutsideRange &&
            let Poll::Ready(result) = self.delete_outside_range.poll_unpin(cx)
        {
            match result {
                Ok(_) => self.recover_index(),
                Err(e) => {
                    self.delete_outside_range();

                    return Poll::Ready(Some(Err(e)));
                }
            }
        }

        if self.state == State::RecoverIndex &&
            let Poll::Ready(result) = self.recover_index.poll_unpin(cx)
        {
            match result {
                Ok(last) => {
                    self.last = last;
                    self.count_files();
                }
                Err(error) => {
                    self.recover_index();
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }

        if self.state == State::CountFiles &&
            let Poll::Ready(result) = self.files_count.poll_unpin(cx)
        {
            let downloaded = match result {
                Ok(downloaded) => downloaded,
                Err(error) => {
                    self.count_files();
                    return Poll::Ready(Some(Err(error)));
                }
            };
            let max_missing = self
                .max_files
                .saturating_sub(downloaded + self.downloading)
                .max(self.last.unwrap_or_default().saturating_sub(self.index));
            self.state = State::Missing(max_missing);
        }

        if let State::Missing(max_missing) = self.state {
            if max_missing > 0 {
                let index = self.index;
                self.index += 1;
                self.next_url(index, max_missing);
            } else {
                self.count_files();
            }
        }

        if let State::NextUrl(max_missing) = self.state &&
            let Poll::Ready(url) = self.next_url.poll_unpin(cx)
        {
            self.state = State::Missing(max_missing - 1);

            return Poll::Ready(url.transpose().map(|url| {
                url.map(|url| -> DownloadFuture {
                    self.downloading += 1;
                    let mut client = self.client.clone();
                    Box::pin(
                        async move { client.download_to_file(url).await.map(EraRemoteMeta::new) },
                    )
                })
            }));
        }

        Poll::Pending
    }
}

impl<Http> StartingStream<Http> {
    const fn downloaded(&mut self) {
        self.downloading = self.downloading.saturating_sub(1);
    }
}

impl<Http: HttpClient + Clone + Send + Sync + 'static> StartingStream<Http> {
    fn fetch_file_list(&mut self) {
        let client = self.client.clone();

        Pin::new(&mut self.fetch_file_list)
            .set(Box::pin(async move { client.fetch_file_list().await }));

        self.state = State::FetchFileList;
    }

    fn delete_outside_range(&mut self) {
        let index = self.index;
        let max_files = self.max_files;
        let client = self.client.clone();

        Pin::new(&mut self.delete_outside_range)
            .set(Box::pin(async move { client.delete_outside_range(index, max_files).await }));

        self.state = State::DeleteOutsideRange;
    }

    fn recover_index(&mut self) {
        let client = self.client.clone();

        Pin::new(&mut self.recover_index)
            .set(Box::pin(async move { client.recover_index().await }));

        self.state = State::RecoverIndex;
    }

    fn count_files(&mut self) {
        let client = self.client.clone();

        Pin::new(&mut self.files_count).set(Box::pin(async move { client.files_count().await }));

        self.state = State::CountFiles;
    }

    fn next_url(&mut self, index: usize, max_missing: usize) {
        let client = self.client.clone();

        Pin::new(&mut self.next_url).set(Box::pin(async move { client.url(index).await }));

        self.state = State::NextUrl(max_missing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::task::noop_waker;
    use reqwest::Client;
    use std::path::PathBuf;

    #[test]
    #[should_panic(expected = "maximum cached files must be nonzero")]
    fn rejects_zero_cache_capacity() {
        EraStreamConfig::default().with_max_files(0);
    }

    #[test]
    #[should_panic(expected = "maximum concurrent downloads must be nonzero")]
    fn rejects_zero_download_concurrency() {
        EraStreamConfig::default().with_max_concurrent_downloads(0);
    }

    #[test]
    fn cache_scan_errors_do_not_release_pending_download_capacity() {
        let client = EraClient::new(
            Client::new(),
            Url::parse("https://example.com/").unwrap(),
            PathBuf::new(),
        );
        let mut stream = EraStream::new(client, EraStreamConfig::default().with_max_files(2));
        stream.starting_stream.downloading = 2;
        stream.download_stream.downloads.push_back(Box::pin(std::future::pending()));
        stream.download_stream.downloads.push_back(Box::pin(std::future::pending()));
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        for _ in 0..3 {
            stream.starting_stream.state = State::CountFiles;
            stream.starting_stream.files_count =
                Box::pin(async { Err(eyre::eyre!("cache scan failed")) });
            let Poll::Ready(Some(Err(error))) = Pin::new(&mut stream).poll_next(&mut context)
            else {
                panic!("cache failure must be delivered without waiting for earlier downloads");
            };
            assert_eq!(error.to_string(), "cache scan failed");
            assert_eq!(stream.starting_stream.downloading, 2);
            assert_eq!(stream.download_stream.downloads.len(), 2);
            assert!(stream.download_stream.scheduled.is_empty());
        }
    }
}
