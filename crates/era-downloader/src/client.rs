use futures_util::stream::{FuturesOrdered, Stream, StreamExt};
use pin_project::pin_project;
use reqwest::{Client, IntoUrl, Url};
use std::{
    future::Future,
    path::Path,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};
use tokio::fs::File;

#[derive(Debug, Clone)]
pub struct EraClient {
    client: Client,
    url: Url,
    folder: Box<Path>,
}

impl EraClient {
    pub async fn download_to_file(&mut self, url: impl IntoUrl) -> eyre::Result<Box<Path>> {
        let path = self.folder.to_path_buf();

        let url = url.into_url()?;
        let client = self.client.clone();
        let file_name = url.path_segments().unwrap().last().unwrap();
        let path = path.join(file_name);

        let response = client.get(url).send().await?;

        let mut stream = response.bytes_stream();
        let mut file = File::create(&path).await?;

        while let Some(item) = stream.next().await {
            tokio::io::copy(&mut item?.as_ref(), &mut file).await?;
        }

        Ok(path.into_boxed_path())
    }

    async fn next_url(&self) -> Option<Url> {
        let mut max = None;

        for mut dir in tokio::fs::read_dir(&self.folder).await {
            for entry in dir.next_entry().await {
                if let Some(entry) = entry {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Some(number) = self.file_name_to_number(name) {
                            if max.is_none() || number > max.unwrap() {
                                max.replace(number);
                            }
                        }
                    }
                }
            }
        }

        let number = max.unwrap_or(0);

        Some(Url::from_str(&self.number_to_file_name(number)).unwrap())
    }

    async fn files_count(&self) -> usize {
        tokio::fs::read_dir(&self.folder).await.iter().count()
    }

    fn number_to_file_name(&self, number: u64) -> String {
        String::new()
    }

    fn file_name_to_number(&self, file_name: &str) -> Option<u64> {
        file_name.split("-").skip(1).next().map(|v| u64::from_str(v).ok()).flatten()
    }
}

struct EraStream {
    pub download_stream: DownloadStream,
    pub starting_stream: StartingStream,
}

impl Stream for EraStream {
    type Item = eyre::Result<Box<Path>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.starting_stream).poll_next(cx) {
            Poll::Ready(fut) => match fut {
                Some(fut) => self.download_stream.downloads.push_back(fut),
                None => {}
            },
            Poll::Pending => {}
        }

        let downloading = self.download_stream.downloads.len();
        self.starting_stream.downloading(downloading);

        Pin::new(&mut self.download_stream).poll_next(cx)
    }
}

#[pin_project]
struct DownloadStream {
    #[pin]
    pub downloads: FuturesOrdered<Pin<Box<dyn Future<Output = eyre::Result<Box<Path>>>>>>,
}

impl Stream for DownloadStream {
    type Item = eyre::Result<Box<Path>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let project = self.project();
        project.downloads.poll_next(cx)
    }
}

#[pin_project]
struct StartingStream {
    client: EraClient,
    filler: Pin<Box<dyn Future<Output = ()>>>,
    downloading: usize,
    files_count: Pin<Box<dyn Future<Output = usize> + Send + Sync + 'static>>,
    next_url: Pin<Box<dyn Future<Output = Option<Url>> + Send + Sync + 'static>>,
    files_count_ready: bool,
    state: State,
    max_files: usize,
    max_concurrent_downloads: usize,
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

        if self.state == State::Initial {
            if self.files_count_ready {
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
