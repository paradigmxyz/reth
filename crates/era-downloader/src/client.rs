use alloy_primitives::{hex, hex::ToHexExt};
use bytes::Bytes;
use eyre::{eyre, OptionExt};
use futures_util::{stream::StreamExt, Stream, TryStreamExt};
use reqwest::{Client, IntoUrl, Url};
use sha2::{Digest, Sha256};
use std::{future::Future, path::Path, str::FromStr};
use tokio::{
    fs::{self, File},
    io::{self, AsyncBufReadExt, AsyncWriteExt},
    join, try_join,
};

/// Accesses the network over HTTP.
pub trait HttpClient {
    /// Makes an HTTP GET request to `url`. Returns a stream of response body bytes.
    fn get<U: IntoUrl + Send + Sync>(
        &self,
        url: U,
    ) -> impl Future<
        Output = eyre::Result<impl Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>,
    > + Send
           + Sync;
}

impl HttpClient for Client {
    async fn get<U: IntoUrl + Send + Sync>(
        &self,
        url: U,
    ) -> eyre::Result<impl Stream<Item = eyre::Result<Bytes>> + Unpin> {
        let response = Self::get(self, url).send().await?;

        Ok(response.bytes_stream().map_err(|e| eyre::Error::new(e)))
    }
}

/// An HTTP client with features for downloading ERA files from an external HTTP accessible
/// endpoint.
#[derive(Debug, Clone)]
pub struct EraClient<Http> {
    client: Http,
    url: Url,
    folder: Box<Path>,
}

impl<Http: HttpClient + Clone> EraClient<Http> {
    const CHECKSUMS: &'static str = "checksums.txt";

    /// Constructs [`EraClient`] using `client` to download from `url` into `folder`.
    pub const fn new(client: Http, url: Url, folder: Box<Path>) -> Self {
        Self { client, url, folder }
    }

    /// Performs a GET request on `url` and stores the response body into a file located within
    /// the `folder`.
    pub async fn download_to_file(&mut self, url: impl IntoUrl) -> eyre::Result<Box<Path>> {
        let path = self.folder.to_path_buf();

        let url = url.into_url()?;
        let client = self.client.clone();
        let file_name = url
            .path_segments()
            .ok_or_eyre("cannot-be-a-base")?
            .next_back()
            .ok_or_eyre("empty path segments")?;
        let path = path.join(file_name);

        let number =
            self.file_name_to_number(file_name).ok_or_eyre("Cannot parse number from file name")?;
        let mut stream = client.get(url).await?;
        let mut file = File::create(&path).await?;
        let mut hasher = Sha256::new();

        while let Some(item) = stream.next().await.transpose()? {
            io::copy(&mut item.as_ref(), &mut file).await?;
            hasher.update(item);
        }

        let actual_checksum = hasher.finalize().to_vec();

        let file = File::open(self.folder.join(Self::CHECKSUMS)).await?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();

        for _ in 0..number {
            lines.next_line().await?;
        }
        let expected_checksum =
            lines.next_line().await?.ok_or_else(|| eyre!("Missing hash for number {number}"))?;
        let expected_checksum = hex::decode(expected_checksum)?;

        if actual_checksum != expected_checksum {
            return Err(eyre!(
                "Checksum mismatch, got: {}, expected: {}",
                actual_checksum.encode_hex(),
                expected_checksum.encode_hex()
            ));
        }

        Ok(path.into_boxed_path())
    }

    /// Recovers index of file following the latest downloaded file from a different run.
    pub async fn recover_index(&self) -> u64 {
        let mut max = None;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(number) = self.file_name_to_number(name) {
                        if max.is_none() || matches!(max, Some(max) if number > max) {
                            max.replace(number);
                        }
                    }
                }
            }
        }

        max.map(|v| v + 1).unwrap_or(0)
    }

    /// Returns a download URL for the file corresponding to `number`.
    pub async fn url(&self, number: u64) -> eyre::Result<Option<Url>> {
        Ok(self.number_to_file_name(number).await?.map(|name| self.url.join(&name)).transpose()?)
    }

    /// Returns the number of files in the `folder`.
    pub async fn files_count(&self) -> usize {
        let mut count = 0usize;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                if entry.path().extension() == Some("era1".as_ref()) {
                    count += 1;
                }
            }
        }

        count
    }

    /// Fetches the list of ERA1 files from `url` and stores it in a file located within `folder`.
    pub async fn fetch_file_list(&self) -> eyre::Result<()> {
        let (mut index, mut checksums) = try_join!(
            self.client.get(self.url.clone()),
            self.client.get(self.url.clone().join(Self::CHECKSUMS)?),
        )?;

        let index_path = self.folder.to_path_buf().join("index.html");
        let checksums_path = self.folder.to_path_buf().join(Self::CHECKSUMS);

        let (mut index_file, mut checksums_file) =
            try_join!(File::create(&index_path), File::create(&checksums_path))?;

        loop {
            let (index, checksums) = join!(index.next(), checksums.next());
            let (index, checksums) = (index.transpose()?, checksums.transpose()?);

            if index.is_none() && checksums.is_none() {
                break;
            }
            let index_file = &mut index_file;
            let checksums_file = &mut checksums_file;

            try_join!(
                async move {
                    if let Some(index) = index {
                        io::copy(&mut index.as_ref(), index_file).await?;
                    }
                    Ok::<(), eyre::Error>(())
                },
                async move {
                    if let Some(checksums) = checksums {
                        io::copy(&mut checksums.as_ref(), checksums_file).await?;
                    }
                    Ok::<(), eyre::Error>(())
                },
            )?;
        }

        let file = File::open(&index_path).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use test_case::test_case;

    impl EraClient<Client> {
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
