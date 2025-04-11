use crate::EraClient;
use futures_util::{stream::FuturesOrdered, FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use reqwest::Url;
use std::{
    collections::VecDeque,
    fmt::{Debug, Formatter},
    future::Future,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};

/// An asynchronous stream of ERA1 files.
#[derive(Debug)]
pub struct EraStream {
    download_stream: DownloadStream,
    starting_stream: StartingStream,
}

impl EraStream {
    /// Constructs a new [`EraStream`] that downloads concurrently up to `max_concurrent_downloads`
    /// ERA1 files to `client` `folder`, keeping their count up to `max_files`.
    pub fn new(client: EraClient, max_files: usize, max_concurrent_downloads: usize) -> Self {
        Self {
            download_stream: DownloadStream {
                downloads: Default::default(),
                scheduled: Default::default(),
                max_concurrent_downloads,
                ended: false,
            },
            starting_stream: StartingStream {
                client,
                files_count: Box::pin(async move { usize::MAX }),
                next_url: Box::pin(async move { None }),
                recover_index: Box::pin(async move { 0 }),
                state: Default::default(),
                max_files,
                max_concurrent_downloads,
                index: 0,
                downloading: 0,
            },
        }
    }
}

impl Stream for EraStream {
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

#[pin_project]
struct DownloadStream {
    #[pin]
    pub downloads: FuturesOrdered<DownloadFuture>,
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
        let project = self.project();
        let poll = project.downloads.poll_next(cx);

        if let Poll::Ready(None) = poll {
            if !ended {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }

        poll
    }
}

struct StartingStream {
    client: EraClient,
    files_count: Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>,
    next_url: Pin<Box<dyn Future<Output = Option<Url>> + Send + Sync + 'static>>,
    recover_index: Pin<Box<dyn Future<Output = u64> + Send + Sync + 'static>>,
    state: State,
    max_files: usize,
    max_concurrent_downloads: usize,
    index: u64,
    downloading: usize,
}

impl Debug for StartingStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StartingStream{{ max_files: {}, max_concurrent_downloads: {} }}",
            self.max_files, self.max_concurrent_downloads
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

impl StartingStream {
    fn downloaded(&mut self) {
        self.downloading = self.downloading.saturating_sub(1);
    }
}

impl Stream for StartingStream {
    type Item = Pin<Box<dyn Future<Output = eyre::Result<Box<Path>>>>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.state == State::Initial {
            let client = self.client.clone();

            Pin::new(&mut self.recover_index)
                .set(Box::pin(async move { client.recover_index().await }));

            self.state = State::RecoverIndex;
        }

        if self.state == State::RecoverIndex {
            if let Poll::Ready(index) = self.recover_index.poll_unpin(cx) {
                self.index = index;

                let client = self.client.clone();

                Pin::new(&mut self.files_count)
                    .set(Box::pin(async move { client.files_count().await }));

                self.state = State::CountFiles;
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
                let client = self.client.clone();

                Pin::new(&mut self.next_url)
                    .set(Box::pin(async move { client.next_url(index).await }));

                self.state = State::NextUrl(max_missing);
            } else {
                let client = self.client.clone();

                Pin::new(&mut self.files_count)
                    .set(Box::pin(async move { client.files_count().await }));

                self.state = State::CountFiles;
            }
        }

        if let State::NextUrl(max_missing) = self.state {
            if let Poll::Ready(url) = self.next_url.poll_unpin(cx) {
                self.state = State::Missing(max_missing.saturating_sub(1));

                return Poll::Ready(if let Some(url) = url {
                    let mut client = self.client.clone();

                    Some(Box::pin(async move { client.download_to_file(url).await }))
                } else {
                    None
                });
            }
        }

        Poll::Pending
    }
}
