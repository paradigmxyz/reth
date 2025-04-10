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
use tokio::{
    fs,
    fs::File,
    io,
    io::{AsyncBufReadExt, AsyncWriteExt},
};

/// An HTTP client with features for downloading ERA files from an external HTTP accessible
/// endpoint.
#[derive(Debug, Clone)]
pub struct EraClient {
    client: Client,
    url: Url,
    folder: Box<Path>,
}

impl EraClient {
    /// Constructs [`EraClient`] using `client` to download from `url` into `folder`.
    pub fn new(client: Client, url: Url, folder: Box<Path>) -> Self {
        Self { client, url, folder }
    }
}

impl EraClient {
    /// Performs a GET request on `url` and stores the response body into a file located within
    /// the `folder`.
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

        while let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(entry) = dir.next_entry().await {
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

        self.number_to_file_name(number)
            .await
            .unwrap()
            .map(|name| Url::from_str(&name))
            .transpose()
            .unwrap()
    }

    async fn files_count(&self) -> usize {
        tokio::fs::read_dir(&self.folder).await.iter().count().saturating_sub(2)
    }

    /// Fetches the list of ERA1 files from `url` and stores it in a file located within `folder`.
    pub async fn fetch_file_list(&self) -> eyre::Result<()> {
        let response = self.client.get(self.url.clone()).send().await.unwrap();

        let mut stream = response.bytes_stream();
        let path = self.folder.to_path_buf().join("index.html");
        let mut file = File::create(&path).await?;

        while let Some(item) = stream.next().await {
            io::copy(&mut item?.as_ref(), &mut file).await?;
        }

        let file = File::open(&path).await?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();

        let path = self.folder.to_path_buf().join("index");
        let file = File::create(&path).await?;
        let mut writer = io::BufWriter::new(file);

        while let Some(line) = lines.next_line().await? {
            if let Some(j) = line.find(".era1") {
                if let Some(i) = line.find("\"") {
                    let era = &line[i + 1..j + 5];
                    writer.write_all(era.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                }
            }
        }
        writer.flush().await?;

        Ok(())
    }

    /// Returns ERA1 file name that is ordered at `number`.
    pub async fn number_to_file_name(&self, number: u64) -> eyre::Result<Option<String>> {
        let path = self.folder.to_path_buf().join("index");
        let file = File::open(&path).await?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();
        for _ in 0..number {
            lines.next_line().await?;
        }

        Ok(lines.next_line().await?)
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
