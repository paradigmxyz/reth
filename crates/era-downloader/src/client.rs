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
    fs::{self, File},
    io::{self, AsyncBufReadExt, AsyncWriteExt},
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
        let file_name = url.path_segments().unwrap().next_back().unwrap();
        let path = path.join(file_name);

        let response = client.get(url).send().await?;

        let mut stream = response.bytes_stream();
        let mut file = File::create(&path).await?;

        while let Some(item) = stream.next().await {
            io::copy(&mut item?.as_ref(), &mut file).await?;
        }

        Ok(path.into_boxed_path())
    }

    /// Returns a url for the next file to download.
    pub async fn next_url(&self) -> Option<Url> {
        let mut max = None;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(number) = self.file_name_to_number(name) {
                        if max.is_none() || number > max.unwrap() {
                            max.replace(number);
                        }
                    }
                }
            }
        }

        let number = max.map(|v| v + 1).unwrap_or(0);

        self.number_to_file_name(number)
            .await
            .unwrap()
            .map(|name| self.url.join(&name))
            .transpose()
            .unwrap()
    }

    /// Returns the number of files in the `folder`.
    pub async fn files_count(&self) -> usize {
        let mut count = 0usize;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(_)) = dir.next_entry().await {
                count += 1;
            }
        }

        count.saturating_sub(2)
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
                if let Some(i) = line[..j].rfind(|c: char| !c.is_alphanumeric() && c != '-') {
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
        file_name.split('-').nth(1).and_then(|v| u64::from_str(v).ok())
    }
}

struct EraStream {
    pub download_stream: DownloadStream,
    pub starting_stream: StartingStream,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use test_case::test_case;

    impl EraClient {
        fn empty() -> Self {
            Self::new(
                Client::new(),
                Url::from_str("file:///").unwrap(),
                PathBuf::new().into_boxed_path(),
            )
        }
    }

    #[test_case("mainnet-00600-a81ae85f.era1", Some(600))]
    #[test_case("mainnet-00000-a81ae85f.era1", Some(0))]
    #[test_case("00000-a81ae85f.era1", None)]
    #[test_case("", None)]
    fn test_file_name_to_number(file_name: &str, expected_number: Option<u64>) {
        let client = EraClient::empty();

        let actual_number = client.file_name_to_number(file_name);

        assert_eq!(actual_number, expected_number);
    }
}
