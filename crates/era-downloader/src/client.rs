use alloy_primitives::{hex, hex::ToHexExt};
use bytes::Bytes;
use eyre::{eyre, OptionExt};
use futures_util::{stream::StreamExt, Stream, TryStreamExt};
use reqwest::{Client, IntoUrl, Url};
use reth_era::common::file_ops::EraFileType;
use reth_fs_util::FsPathError;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, future::Future, path::Path, str::FromStr, sync::Arc};
use tokio::{
    fs::{self, File},
    io::{self, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
    try_join,
};

/// Downloaded index page filename
const INDEX_HTML_FILE: &str = "index.html";

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
    era_type: EraFileType,
    file_lists: Arc<Mutex<HashMap<EraFileType, FileList>>>,
}

impl<Http: HttpClient + Clone> EraClient<Http> {
    const CHECKSUMS: &'static str = "checksums.txt";

    /// Constructs [`EraClient`] using `client` to download from `url` into `folder`.
    ///
    /// The file type is auto-detected from the URL. Use
    /// [`with_era_type`](Self::with_era_type) to override.
    pub fn new(client: Http, url: Url, folder: impl Into<Box<Path>>) -> Self {
        let era_type = EraFileType::from_url(url.as_str());
        Self { client, url, folder: folder.into(), era_type, file_lists: Default::default() }
    }

    /// Override the auto-detected [`EraFileType`].
    pub const fn with_era_type(mut self, era_type: EraFileType) -> Self {
        self.era_type = era_type;
        self
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

            // Download to a temp path and rename in only on success, so an interrupted download
            // never leaves a partial file that later looks complete.
            let tmp_path = path.with_extension("tmp");

            let mut tries = 1..3;
            let mut actual_checksum: eyre::Result<_>;
            loop {
                actual_checksum = async {
                    let mut file = File::create(&tmp_path).await?;
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

            if self.era_type.has_checksums() {
                self.assert_checksum(number, actual_checksum?)
                    .await
                    .map_err(|e| eyre!("{e} for {file_name} at {}", path.display()))?;
            } else {
                // No checksum to validate against; surface a failed download before renaming.
                actual_checksum?;
            }

            fs::rename(&tmp_path, &path).await?;
        }

        Ok(path.into_boxed_path())
    }

    /// Recovers index of file following the latest downloaded file from a different run.
    pub async fn recover_index(&self) -> Option<usize> {
        let mut max = None;

        if let Ok(mut dir) = fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Some(name) = entry.file_name().to_str() &&
                    self.is_matching_era_file(name) &&
                    let Some(number) = self.file_name_to_number(name) &&
                    (max.is_none() || matches!(max, Some(max) if number > max))
                {
                    max.replace(number + 1);
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
                if let Some(name) = entry.file_name().to_str() &&
                    self.is_matching_era_file(name) &&
                    let Some(number) = self.file_name_to_number(name) &&
                    (number < index || number >= last)
                {
                    reth_fs_util::remove_file_if_exists(entry.path())?;
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
                if let Some(ext) = entry.path().extension().and_then(|ext| ext.to_str()) &&
                    self.era_type
                        .extensions()
                        .iter()
                        .any(|valid| valid.trim_start_matches('.') == ext)
                {
                    count += 1;
                }
            }
        }

        count
    }

    /// Fetches the list of ERA1/ERA files from `url` and stores it in a file located within
    /// `folder`.
    /// For era files, checksum.txt file does not exist, so the checksum verification is
    /// skipped.
    ///
    /// A successful fetch replaces the filename and checksum snapshot shared with this client's
    /// clones. Later lookups use that snapshot until the next successful fetch; changes made to
    /// sidecar files externally are not observed until then. Failed refreshes retain the last
    /// successful snapshot. Archive types have separate snapshots. Refreshes are serialized across
    /// clones, and lookups wait until an ongoing refresh finishes.
    pub async fn fetch_file_list(&self) -> eyre::Result<()> {
        let mut file_lists = self.file_lists.lock().await;
        let index_path = self.folder.to_path_buf().join(INDEX_HTML_FILE);
        let checksums_path = self.folder.to_path_buf().join(Self::CHECKSUMS);

        // Only for files that ship checksums (era1, ere) we also download the checksums file.
        if self.era_type.has_checksums() {
            let checksums_url = self.url.join(Self::CHECKSUMS)?;
            try_join!(
                self.download_file_to_path(self.url.clone(), &index_path),
                self.download_file_to_path(checksums_url, &checksums_path)
            )?;
        } else {
            // Download only index file
            self.download_file_to_path(self.url.clone(), &index_path).await?;
        }

        // Parse and extract era filenames from index.html
        let file_names = self.extract_era_filenames(&index_path).await?;
        let checksums = if self.era_type.has_checksums() {
            fs::read(&checksums_path)
                .await
                .map_err(|error| FsPathError::read(error, &checksums_path))?
                .split_inclusive(|byte| *byte == b'\n')
                .map(|line| {
                    let line = if let Some(line) = line.strip_suffix(b"\n") {
                        line.strip_suffix(b"\r").unwrap_or(line)
                    } else {
                        line
                    };
                    line.to_vec()
                })
                .collect()
        } else {
            Vec::new()
        };
        file_lists.insert(self.era_type, FileList { file_names, checksums });
        Ok(())
    }

    /// Extracts ERA filenames from `index.html` and writes them to the index file
    async fn extract_era_filenames(&self, index_path: &Path) -> eyre::Result<Vec<String>> {
        let file = File::open(index_path).await?;
        let reader = io::BufReader::new(file);
        let mut lines = reader.lines();

        let path = self.folder.to_path_buf().join("index");
        let file = File::create(&path).await?;
        let mut writer = io::BufWriter::new(file);
        let mut file_names = Vec::new();

        while let Some(line) = lines.next_line().await? {
            if let Some(era) = extract_era_filename(&line, self.era_type.extensions()) {
                writer.write_all(era.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                file_names.push(era.to_owned());
            }
        }

        writer.flush().await?;
        Ok(file_names)
    }

    // Helper to download a file to a specified path
    async fn download_file_to_path(&self, url: Url, path: &Path) -> eyre::Result<()> {
        let mut stream = self.client.get(url).await?;
        let mut file = File::create(path).await?;

        while let Some(item) = stream.next().await.transpose()? {
            io::copy(&mut item.as_ref(), &mut file).await?;
        }

        Ok(())
    }

    /// Returns ERA1/ERA file name that is ordered at `number`.
    pub async fn number_to_file_name(&self, number: usize) -> eyre::Result<Option<String>> {
        if let Some(file_list) = self.file_lists.lock().await.get(&self.era_type) {
            return Ok(file_list.file_names.get(number).cloned());
        }
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
                if self.era_type.has_checksums() {
                    let number = self
                        .file_name_to_number(name)
                        .ok_or_else(|| eyre!("Cannot parse ERA number from {name}"))?;

                    let actual_checksum = checksum(file).await?;
                    let is_verified = self.verify_checksum(number, actual_checksum).await?;

                    if !is_verified {
                        fs::remove_file(path).await?;
                    }

                    Ok(is_verified)
                } else {
                    // For era files there is no checksums.txt, so verification is skipped.
                    Ok(true)
                }
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
        if let Some(file_list) = self.file_lists.lock().await.get(&self.era_type) {
            let checksum = file_list
                .checksums
                .get(number)
                .ok_or_else(|| eyre!("Missing hash for number {number}"))?;
            return Ok(hex::decode(checksum)?);
        }
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

    /// Whether `file_name` is a downloaded ERA file of this client's configured type.
    ///
    /// Excludes partial (`*.tmp`) and sidecar files that share the `<network>-<number>-...` stem,
    /// so they don't influence resume or cleanup.
    fn is_matching_era_file(&self, file_name: &str) -> bool {
        EraFileType::from_filename(file_name) == Some(self.era_type)
    }
}

/// The filenames and raw checksum lines from one successful file-list fetch.
#[derive(Debug)]
struct FileList {
    file_names: Vec<String>,
    checksums: Vec<Vec<u8>>,
}

/// Extracts an era filename ending in one of `extensions` from a single index line.
///
/// `extensions` are tried in order; pass them longest-first so `.ere` never matches inside `.erae`.
fn extract_era_filename<'a>(line: &'a str, extensions: &[&str]) -> Option<&'a str> {
    for ext in extensions {
        if let Some(j) = line.find(ext) &&
            let Some(i) = line[..j].rfind(|c: char| !c.is_alphanumeric() && c != '-')
        {
            return Some(&line[i + 1..j + ext.len()]);
        }
    }
    None
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
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
    };
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

    // `.erae` lines must yield the full `.erae` name, never the `.ere` prefix inside it.
    #[test_case(
        "<a href=\"mainnet-00000-a6860fef.erae\">", &[".erae", ".ere"],
        Some("mainnet-00000-a6860fef.erae"); "erae anchor not clipped to ere"
    )]
    #[test_case(
        "    \"name\": \"mainnet-00001-05c64fc4.erae\",", &[".erae", ".ere"],
        Some("mainnet-00001-05c64fc4.erae"); "erae json entry"
    )]
    #[test_case(
        "<a href=\"mainnet-00600-a81ae85f.era1\">", &[".era1"],
        Some("mainnet-00600-a81ae85f.era1"); "era1 anchor"
    )]
    #[test_case("<a href=\"checksums.txt\">", &[".erae", ".ere"], None; "no era file on line")]
    fn test_extract_era_filename(line: &str, exts: &[&str], expected: Option<&str>) {
        assert_eq!(extract_era_filename(line, exts), expected);
    }

    #[test]
    fn test_with_era_type_overrides_auto_detection() {
        // URL without "era1" auto-detects as Era
        let client = EraClient::new(
            Client::new(),
            Url::from_str("https://example.com/").unwrap(),
            PathBuf::new(),
        );
        assert_eq!(client.era_type, EraFileType::Era);

        // with_era_type overrides to Era1
        let client = client.with_era_type(EraFileType::Era1);
        assert_eq!(client.era_type, EraFileType::Era1);
    }

    #[derive(Clone, Debug, Default)]
    struct ListingClient {
        index: Arc<Mutex<String>>,
        checksums: Arc<Mutex<Vec<u8>>>,
        fail: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }

    impl HttpClient for ListingClient {
        async fn get<U: IntoUrl + Send + Sync>(
            &self,
            url: U,
        ) -> eyre::Result<impl Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail.load(Ordering::Relaxed) {
                return Err(eyre!("metadata fetch failed"));
            }
            let url = url.into_url()?;
            let body = if url.path().ends_with("checksums.txt") {
                self.checksums.lock().await.clone()
            } else {
                self.index.lock().await.clone().into_bytes()
            };
            Ok(futures_util::stream::iter([Ok(Bytes::from(body))]))
        }
    }

    #[tokio::test]
    async fn file_list_refresh_updates_clones_and_preserves_lazy_errors() {
        let folder = tempfile::tempdir().unwrap();
        let http = ListingClient::default();
        *http.index.lock().await = "<a href=\"mainnet-00000-original.era1\">\n".to_owned();
        *http.checksums.lock().await = b"00\r\n\xff\n".to_vec();
        let client = EraClient::new(
            http.clone(),
            Url::parse("https://example.com/era1/").unwrap(),
            folder.path(),
        );
        let clone = client.clone();
        client.fetch_file_list().await.unwrap();
        assert_eq!(
            clone.number_to_file_name(0).await.unwrap().as_deref(),
            Some("mainnet-00000-original.era1")
        );
        assert_eq!(clone.expected_checksum(0).await.unwrap(), vec![0]);
        assert_eq!(
            clone.expected_checksum(1).await.unwrap_err().downcast_ref::<hex::FromHexError>(),
            Some(&hex::FromHexError::OddLength)
        );

        *http.index.lock().await =
            "<a href=\"mainnet-00000-revised-name.era1\">\n<a href=\"mainnet-00001-next.era1\">\n"
                .to_owned();
        *http.checksums.lock().await = b"02\n03\n".to_vec();
        http.fail.store(true, Ordering::Relaxed);
        assert_eq!(
            client.fetch_file_list().await.unwrap_err().to_string(),
            "metadata fetch failed"
        );
        assert_eq!(
            clone.number_to_file_name(0).await.unwrap().as_deref(),
            Some("mainnet-00000-original.era1")
        );
        assert_eq!(clone.expected_checksum(0).await.unwrap(), vec![0]);

        http.fail.store(false, Ordering::Relaxed);
        client.fetch_file_list().await.unwrap();
        assert_eq!(
            clone.number_to_file_name(0).await.unwrap().as_deref(),
            Some("mainnet-00000-revised-name.era1")
        );
        assert_eq!(
            clone.number_to_file_name(1).await.unwrap().as_deref(),
            Some("mainnet-00001-next.era1")
        );
        assert_eq!(clone.number_to_file_name(2).await.unwrap(), None);
        assert_eq!(clone.expected_checksum(0).await.unwrap(), vec![2]);
        assert_eq!(clone.expected_checksum(1).await.unwrap(), vec![3]);
        assert_eq!(
            clone.expected_checksum(2).await.unwrap_err().to_string(),
            "Missing hash for number 2"
        );
    }

    #[tokio::test]
    async fn changing_archive_type_uses_a_separate_file_list() {
        let folder = tempfile::tempdir().unwrap();
        let http = ListingClient::default();
        *http.index.lock().await = "<a href=\"mainnet-00000-original.era1\">\n".to_owned();
        *http.checksums.lock().await = b"00\n".to_vec();
        let client = EraClient::new(
            http.clone(),
            Url::parse("https://example.com/era1/").unwrap(),
            folder.path(),
        );
        client.fetch_file_list().await.unwrap();
        let era_client = client.clone().with_era_type(EraFileType::Era);
        *http.index.lock().await = "<a href=\"mainnet-00000-consensus.era\">\n".to_owned();
        era_client.fetch_file_list().await.unwrap();
        assert_eq!(
            era_client.number_to_file_name(0).await.unwrap().as_deref(),
            Some("mainnet-00000-consensus.era")
        );
        assert_eq!(
            client.number_to_file_name(0).await.unwrap().as_deref(),
            Some("mainnet-00000-original.era1")
        );
        assert_eq!(client.expected_checksum(0).await.unwrap(), vec![0]);
    }

    #[tokio::test]
    async fn lookups_without_fetch_use_existing_sidecars() {
        let folder = tempfile::tempdir().unwrap();
        reth_fs_util::write(folder.path().join("index"), b"first\nsecond-longer-name\n").unwrap();
        reth_fs_util::write(folder.path().join(EraClient::<Client>::CHECKSUMS), b"00\n01\n")
            .unwrap();
        let client = EraClient::new(
            Client::new(),
            Url::parse("https://example.com/era1/").unwrap(),
            folder.path(),
        );
        assert_eq!(
            client.number_to_file_name(1).await.unwrap().as_deref(),
            Some("second-longer-name")
        );
        assert_eq!(client.number_to_file_name(2).await.unwrap(), None);
        assert_eq!(client.expected_checksum(1).await.unwrap(), vec![1]);
        assert_eq!(
            client.expected_checksum(2).await.unwrap_err().to_string(),
            "Missing hash for number 2"
        );
    }

    #[tokio::test]
    async fn concurrent_refreshes_do_not_interleave_sidecars() {
        let folder = tempfile::tempdir().unwrap();
        let http = ListingClient::default();
        *http.index.lock().await = "<a href=\"mainnet-00000-original.era1\">\n".to_owned();
        *http.checksums.lock().await = b"00\n".to_vec();
        let client = EraClient::new(
            http.clone(),
            Url::parse("https://example.com/era1/").unwrap(),
            folder.path(),
        );
        let clone = client.clone();
        let index_guard = http.index.lock().await;
        let mut first = Box::pin(client.fetch_file_list());
        assert!(futures::poll!(&mut first).is_pending());
        assert_eq!(http.calls.load(Ordering::Relaxed), 2);
        let mut second = Box::pin(clone.fetch_file_list());
        assert!(futures::poll!(&mut second).is_pending());
        assert_eq!(
            http.calls.load(Ordering::Relaxed),
            2,
            "second refresh must wait before downloading sidecars"
        );
        drop(index_guard);
        let (first, second) = tokio::join!(first, second);
        first.unwrap();
        second.unwrap();
        assert_eq!(http.calls.load(Ordering::Relaxed), 4);
        assert_eq!(clone.expected_checksum(0).await.unwrap(), vec![0]);
    }
}
