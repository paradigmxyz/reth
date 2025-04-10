use crate::EraClient;
use futures_util::{stream::FuturesOrdered, Stream};
use pin_project::pin_project;
use reqwest::Url;
use std::{
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
            download_stream: DownloadStream { downloads: Default::default() },
            starting_stream: StartingStream {
                client,
                downloading: 0,
                files_count: Box::pin(async move { 0 }),
                next_url: Box::pin(async move { None }),
                files_count_ready: false,
                state: State::Initial,
                max_files,
                max_concurrent_downloads,
            },
        }
    }
}

impl Stream for EraStream {
    type Item = eyre::Result<Box<Path>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Poll::Ready(Some(fut)) = Pin::new(&mut self.starting_stream).poll_next(cx) {
            self.download_stream.downloads.push_back(fut);
        }

        let downloading = self.download_stream.downloads.len();
        self.starting_stream.downloading(downloading);

        Pin::new(&mut self.download_stream).poll_next(cx)
    }
}

type DownloadFuture = Pin<Box<dyn Future<Output = eyre::Result<Box<Path>>>>>;

#[pin_project]
struct DownloadStream {
    #[pin]
    pub downloads: FuturesOrdered<DownloadFuture>,
}

impl Debug for DownloadStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DownloadStream({})", self.downloads.len())
    }
}

impl Stream for DownloadStream {
    type Item = eyre::Result<Box<Path>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let project = self.project();
        project.downloads.poll_next(cx)
    }
}

struct StartingStream {
    client: EraClient,
    downloading: usize,
    files_count: Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>,
    next_url: Pin<Box<dyn Future<Output = Option<Url>> + Send + Sync + 'static>>,
    files_count_ready: bool,
    state: State,
    max_files: usize,
    max_concurrent_downloads: usize,
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

#[derive(Debug, PartialEq)]
enum State {
    Initial,
    Missing(usize),
    NextUrl(usize),
}

impl StartingStream {
    fn downloading(&mut self, downloading: usize) {
        self.downloading = downloading;

        if !self.files_count_ready {
            let client = self.client.clone();

            Pin::new(&mut self.files_count)
                .set(Box::pin(async move { client.files_count().await }));
            self.files_count_ready = true;
        }
    }
}

impl Stream for StartingStream {
    type Item = Pin<Box<dyn Future<Output = eyre::Result<Box<Path>>>>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let downloading = self.downloading;

        if self.state == State::Initial && self.files_count_ready {
            match Pin::new(&mut self.files_count).poll(cx) {
                Poll::Ready(downloaded) => {
                    let max_missing = (downloaded + downloading)
                        .saturating_sub(self.max_files)
                        .max(self.max_concurrent_downloads);

                    self.files_count_ready = false;
                    self.state = State::Missing(max_missing);
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if let State::Missing(max_missing) = self.state {
            if max_missing > 0 {
                let client = self.client.clone();

                Pin::new(&mut self.next_url).set(Box::pin(async move { client.next_url().await }));

                self.state = State::NextUrl(max_missing);
            } else {
                self.state = State::Initial;
            }
        }

        if let State::NextUrl(max_missing) = self.state {
            return match Pin::new(&mut self.next_url).poll(cx) {
                Poll::Ready(url) => {
                    self.state = State::Missing(max_missing.saturating_sub(1));

                    if let Some(url) = url {
                        let mut client = self.client.clone();

                        Poll::Ready(Some(Box::pin(
                            async move { client.download_to_file(url).await },
                        )))
                    } else {
                        Poll::Ready(None)
                    }
                }
                Poll::Pending => Poll::Pending,
            }
        }

        Poll::Pending
    }
}
