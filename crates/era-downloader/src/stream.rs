use crate::{client::HttpClient, EraClient};
use futures_util::{stream::FuturesOrdered, FutureExt, Stream, StreamExt};
use reqwest::Url;
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
}

impl Default for EraStreamConfig {
    fn default() -> Self {
        Self { max_files: 5, max_concurrent_downloads: 3 }
    }
}

impl EraStreamConfig {
    /// The maximum amount of downloaded ERA1 files kept in the download directory.
    pub const fn with_max_files(mut self, max_files: usize) -> Self {
        self.max_files = max_files;
        self
    }

    /// The maximum amount of downloads happening at the same time.
    pub const fn with_max_concurrent_downloads(mut self, max_concurrent_downloads: usize) -> Self {
        self.max_concurrent_downloads = max_concurrent_downloads;
        self
    }
}

/// An asynchronous stream of ERA1 files.
#[derive(Debug)]
pub struct EraStream<Http> {
    download_stream: DownloadStream,
    starting_stream: StartingStream<Http>,
}

impl<Http> EraStream<Http> {
    /// Constructs a new [`EraStream`] that downloads concurrently up to `max_concurrent_downloads`
    /// ERA1 files to `client` `folder`, keeping their count up to `max_files`.
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
                files_count: Box::pin(async move { usize::MAX }),
                next_url: Box::pin(async move { Ok(None) }),
                recover_index: Box::pin(async move { 0 }),
                state: Default::default(),
                max_files: config.max_files,
                index: 0,
                downloading: 0,
            },
        }
    }
}

impl<Http: HttpClient + Clone + Send + Sync + 'static + Unpin> Stream for EraStream<Http> {
    type Item = eyre::Result<Box<Path>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Poll::Ready(fut) = self.starting_stream.poll_next_unpin(cx) {
            if let Some(fut) = fut {
                self.download_stream.scheduled.push_back(fut);
            } else {
                self.download_stream.ended = true;
            }
        }

        let poll = self.download_stream.poll_next_unpin(cx);

        if poll.is_ready() {
            self.starting_stream.downloaded();
        }

        poll
    }
}

type DownloadFuture = Pin<Box<dyn Future<Output = eyre::Result<Box<Path>>>>>;

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
    type Item = eyre::Result<Box<Path>>;

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
    files_count: Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>,
    next_url: Pin<Box<dyn Future<Output = eyre::Result<Option<Url>>> + Send + Sync + 'static>>,
    recover_index: Pin<Box<dyn Future<Output = u64> + Send + Sync + 'static>>,
    state: State,
    max_files: usize,
    index: u64,
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
    RecoverIndex,
    CountFiles,
    Missing(usize),
    NextUrl(usize),
}

impl<Http: HttpClient + Clone + Send + Sync + 'static + Unpin> Stream for StartingStream<Http> {
    type Item = Pin<Box<dyn Future<Output = eyre::Result<Box<Path>>>>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.state == State::Initial {
            self.recover_index();
        }

        if self.state == State::RecoverIndex {
            if let Poll::Ready(index) = self.recover_index.poll_unpin(cx) {
                self.index = index;
                self.count_files();
            }
        }

        if self.state == State::CountFiles {
            if let Poll::Ready(downloaded) = self.files_count.poll_unpin(cx) {
                let max_missing = self.max_files.saturating_sub(downloaded + self.downloading);
                self.state = State::Missing(max_missing);
            }
        }

        if let State::Missing(max_missing) = self.state {
            if max_missing > 0 {
                let index = self.index;
                self.index += 1;
                self.downloading += 1;
                self.next_url(index, max_missing);
            } else {
                self.count_files();
            }
        }

        if let State::NextUrl(max_missing) = self.state {
            if let Poll::Ready(url) = self.next_url.poll_unpin(cx) {
                self.state = State::Missing(max_missing - 1);

                return Poll::Ready(url.transpose().map(|url| -> DownloadFuture {
                    let mut client = self.client.clone();

                    Box::pin(async move { client.download_to_file(url?).await })
                }));
            }
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

    fn next_url(&mut self, index: u64, max_missing: usize) {
        let client = self.client.clone();

        Pin::new(&mut self.next_url).set(Box::pin(async move { client.url(index).await }));

        self.state = State::NextUrl(max_missing);
    }
}
