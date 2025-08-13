use alloy_primitives::{hex, hex::ToHexExt};
use bytes::Bytes;
use eyre::{eyre, OptionExt};
use futures_util::{stream::StreamExt, Stream, TryStreamExt};
use reqwest::{Client, IntoUrl, Url};
use sha2::{Digest, Sha256};
use std::{future::Future, path::Path, str::FromStr};
use tokio::{
    fs::{self, File},
    io::{self, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt},
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
    pub fn new(client: Http, url: Url, folder: impl Into<Box<Path>>) -> Self {
        Self { client, url, folder: folder.into() }
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

        if !self.is_downloaded(file_name, &path).await? {
            let number = self
                .file_name_to_number(file_name)
                .ok_or_eyre("Cannot parse number from file name")?;

            let mut tries = 1..3;
            let mut actual_checksum: eyre::Result<_>;
            loop {
                actual_checksum = async {
                    let mut file = File::create(&path).await?;
                    let mut stream = client.get(url.clone()).await?;
                    let mut hasher = Sha256::new();

                    while let Some(item) = stream.next().await.transpose()? {
                        io::copy(&mut item.as_ref(), &mut file).await?;
                        hasher.update(item);
                    }

                    Ok(hasher.finalize().to_vec())
                }
                .await;

                if actual_checksum.is_ok() || tries.next().is_none() {
                    break;
                }
            }

            self.assert_checksum(number, actual_checksum?)
                .await
                .map_err(|e| eyre!("{e} for {file_name} at {}", path.display()))?;
        }

        Ok(path.into_boxed_path())
    }

    /// Recovers index of file following the latest downloaded file from a different run.
    pub async fn recover_index(&self) -> Option<usize> {
        let mut max = None;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(number) = self.file_name_to_number(name) {
                        if max.is_none() || matches!(max, Some(max) if number > max) {
                            max.replace(number + 1);
                        }
                    }
                }
            }
        }

        max
    }

    /// Deletes files that are outside-of the working range.
    pub async fn delete_outside_range(&self, index: usize, max_files: usize) -> eyre::Result<()> {
        let last = index + max_files;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(number) = self.file_name_to_number(name) {
                        if number < index || number >= last {
                            eprintln!("Deleting file {}", entry.path().display());
                            eprintln!("{number} < {index} || {number} > {last}");
                            reth_fs_util::remove_file(entry.path())?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns a download URL for the file corresponding to `number`.
    pub async fn url(&self, number: usize) -> eyre::Result<Option<Url>> {
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
    pub async fn number_to_file_name(&self, number: usize) -> eyre::Result<Option<String>> {
        let path = self.folder.to_path_buf().join("index");
        let file = File::open(&path).await?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();
        for _ in 0..number {
            lines.next_line().await?;
        }

        Ok(lines.next_line().await?)
    }

    async fn is_downloaded(&self, name: &str, path: impl AsRef<Path>) -> eyre::Result<bool> {
        let path = path.as_ref();

        match File::open(path).await {
            Ok(file) => {
                let number = self
                    .file_name_to_number(name)
                    .ok_or_else(|| eyre!("Cannot parse ERA number from {name}"))?;

                let actual_checksum = checksum(file).await?;
                let is_verified = self.verify_checksum(number, actual_checksum).await?;

                if !is_verified {
                    fs::remove_file(path).await?;
                }

                Ok(is_verified)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e)?,
        }
    }

    /// Returns `true` if `actual_checksum` matches expected checksum of the ERA1 file indexed by
    /// `number` based on the [file list].
    ///
    /// [file list]: Self::fetch_file_list
    async fn verify_checksum(&self, number: usize, actual_checksum: Vec<u8>) -> eyre::Result<bool> {
        Ok(actual_checksum == self.expected_checksum(number).await?)
    }

    /// Returns `Ok` if `actual_checksum` matches expected checksum of the ERA1 file indexed by
    /// `number` based on the [file list].
    ///
    /// [file list]: Self::fetch_file_list
    async fn assert_checksum(&self, number: usize, actual_checksum: Vec<u8>) -> eyre::Result<()> {
        let expected_checksum = self.expected_checksum(number).await?;

        if actual_checksum == expected_checksum {
            Ok(())
        } else {
            Err(eyre!(
                "Checksum mismatch, got: {}, expected: {}",
                actual_checksum.encode_hex(),
                expected_checksum.encode_hex()
            ))
        }
    }

    /// Returns SHA-256 checksum for ERA1 file indexed by `number` based on the [file list].
    ///
    /// [file list]: Self::fetch_file_list
    async fn expected_checksum(&self, number: usize) -> eyre::Result<Vec<u8>> {
        let file = File::open(self.folder.join(Self::CHECKSUMS)).await?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();

        for _ in 0..number {
            lines.next_line().await?;
        }
        let expected_checksum =
            lines.next_line().await?.ok_or_else(|| eyre!("Missing hash for number {number}"))?;
        let expected_checksum = hex::decode(expected_checksum)?;

        Ok(expected_checksum)
    }

    fn file_name_to_number(&self, file_name: &str) -> Option<usize> {
        file_name.split('-').nth(1).and_then(|v| usize::from_str(v).ok())
    }
}

async fn checksum(mut reader: impl AsyncRead + Unpin) -> eyre::Result<Vec<u8>> {
    let mut hasher = Sha256::new();

    // Create a buffer to read data into, sized for performance.
    let mut data = vec![0; 64 * 1024];

    loop {
        // Read data from the reader into the buffer.
        let len = reader.read(&mut data).await?;
        if len == 0 {
            break;
        } // Exit loop if no more data.

        // Update the hash with the data read.
        hasher.update(&data[..len]);
    }

    // Finalize the hash after all data has been processed.
    let hash = hasher.finalize().to_vec();

    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use test_case::test_case;

    impl EraClient<Client> {
        fn empty() -> Self {
            Self::new(Client::new(), Url::from_str("file:///").unwrap(), PathBuf::new())
        }
    }

    #[test_case("mainnet-00600-a81ae85f.era1", Some(600))]
    #[test_case("mainnet-00000-a81ae85f.era1", Some(0))]
    #[test_case("00000-a81ae85f.era1", None)]
    #[test_case("", None)]
    fn test_file_name_to_number(file_name: &str, expected_number: Option<usize>) {
        let client = EraClient::empty();

        let actual_number = client.file_name_to_number(file_name);

        assert_eq!(actual_number, expected_number);
    }
}
