use super::error::DownloaderError;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
use tracing::info;

/// Tracks download progress and manages chunked downloads for resumable file transfers.
#[derive(Debug, Serialize, Deserialize)]
pub struct Metadata {
    /// Total file size
    pub total_size: usize,
    /// Total file size
    pub downloaded: usize,
    /// Download chunk size. Default 150MB.
    pub chunk_size: usize,
    /// Remaining download ranges for each chunk.
    /// - `Some((start, end))`: Start and end indices to be downloaded.
    /// - `None`: Chunk fully downloaded.
    chunks: Vec<Option<(usize, usize)>>,
    /// Path with the stored metadata.
    #[serde(skip)]
    path: PathBuf,
}

impl Metadata {
    /// Build a [`Metadata`] using a builder.
    pub fn builder(data_file: &Path) -> MetadataBuilder {
        MetadataBuilder::new(Self::file_path(data_file))
    }

    /// Returns the metadata file path of a data file: `{data_file}.metadata`
    pub fn file_path(data_file: &Path) -> PathBuf {
        data_file.with_file_name(format!(
            "{}.metadata",
            data_file.file_name().unwrap_or_default().to_string_lossy()
        ))
    }

    /// Returns a list of all chunks with their remaining ranges to be downloaded.
    ///
    /// Returns a list of `(chunk_index, (start, end))`
    pub fn needed_ranges(&self) -> Vec<(usize, (usize, usize))> {
        self.chunks
            .iter()
            .enumerate()
            .filter(|(_, remaining)| remaining.is_some())
            .map(|(index, remaining)| (index, remaining.expect("qed")))
            .collect()
    }

    /// Updates a downloaded chunk.
    pub fn update_chunk(
        &mut self,
        index: usize,
        downloaded_bytes: usize,
    ) -> Result<(), DownloaderError> {
        self.downloaded += downloaded_bytes;

        let num_chunks = self.chunks.len();
        if index >= self.chunks.len() {
            return Err(DownloaderError::InvalidChunk(index, num_chunks))
        }

        // Update chunk with downloaded range
        if let Some((mut start, end)) = self.chunks[index] {
            start += downloaded_bytes;
            if start > end {
                self.chunks[index] = None;
            } else {
                self.chunks[index] = Some((start, end));
            }
        }

        let file = self.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        info!(
            target: "sync::stages::s3::downloader",
            file,
            "{}/{}", self.downloaded / 1024 / 1024, self.total_size / 1024 / 1024);

        self.commit()
    }

    /// Commits the [`Metadata`] to file.
    pub fn commit(&self) -> Result<(), DownloaderError> {
        Ok(reth_fs_util::atomic_write_file(&self.path, |file| {
            bincode::serialize_into(file, &self)
        })?)
    }

    /// Loads a [`Metadata`] file from disk using the target data file.
    pub fn load(data_file: &Path) -> Result<Self, DownloaderError> {
        Ok(bincode::deserialize_from(File::open(Self::file_path(data_file))?)?)
    }

    /// Returns true if we have downloaded all chunks.
    pub fn is_done(&self) -> bool {
        !self.chunks.iter().any(|c| c.is_some())
    }

    /// Deletes [`Metadata`] file from disk.
    pub fn delete(self) -> Result<(), DownloaderError> {
        Ok(reth_fs_util::remove_file(&self.path)?)
    }
}

/// A builder that can configure [Metadata]
#[derive(Debug)]
pub struct MetadataBuilder {
    /// Path with the stored metadata.
    metadata_path: PathBuf,
    /// Total file size
    total_size: Option<usize>,
    /// Download chunk size. Default 150MB.
    chunk_size: usize,
}

impl MetadataBuilder {
    const fn new(metadata_path: PathBuf) -> Self {
        Self {
            metadata_path,
            total_size: None,
            chunk_size: 150 * (1024 * 1024), // 150MB
        }
    }

    pub const fn with_total_size(mut self, total_size: usize) -> Self {
        self.total_size = Some(total_size);
        self
    }

    pub const fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Returns a [Metadata] if
    pub fn build(&self) -> Result<Metadata, DownloaderError> {
        match &self.total_size {
            Some(total_size) if *total_size > 0 => {
                let chunks = (0..*total_size)
                    .step_by(self.chunk_size)
                    .map(|start| {
                        Some((start, (start + self.chunk_size).min(*total_size).saturating_sub(1)))
                    })
                    .collect();

                let metadata = Metadata {
                    path: self.metadata_path.clone(),
                    total_size: *total_size,
                    downloaded: 0,
                    chunk_size: self.chunk_size,
                    chunks,
                };
                metadata.commit()?;

                Ok(metadata)
            }
            _ => Err(DownloaderError::InvalidMetadataTotalSize(self.total_size)),
        }
    }
}
