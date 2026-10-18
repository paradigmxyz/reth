//! An asynchronous stream interface for downloading archives into a dedicated, disposable cache.
//!
//! Starting an [`EraStream`] deletes matching cached archives outside its initial configured
//! window, including preexisting unprocessed files. Use a separate directory for archival data.
//! After consuming a downloaded file, close its handles and call [`EraMeta::mark_as_processed`] to
//! delete it and free capacity for the next download. Dropping metadata does not acknowledge it.
//!
//! # Examples
//! ```no_run
//! use futures_util::StreamExt;
//! use reqwest::{Client, Url};
//! use reth_era_downloader::{EraClient, EraMeta, EraStream, EraStreamConfig};
//! use std::{path::PathBuf, str::FromStr};
//!
//! # async fn f() -> eyre::Result<()> {
//! // URL where the ERA1 files are hosted
//! let url = Url::from_str("https://era1.ethportal.net/")?;
//!
//! // Dedicated cache whose matching ERA1 files may be deleted.
//! let folder = PathBuf::from("era-cache");
//! reth_fs_util::create_dir_all(&folder)?;
//!
//! let client = EraClient::new(Client::new(), url, folder);
//!
//! let config = EraStreamConfig::default()
//!     // Keep up to 2 ERA1 files in the `folder`.
//!     // More downloads won't start until some of the files are removed.
//!     .with_max_files(2)
//!     // Do not download more than 2 files at the same time.
//!     .with_max_concurrent_downloads(2);
//!
//! let mut stream = EraStream::new(client, config);
//!
//! while let Some(file) = stream.next().await {
//!     let file = file?;
//!     // Process the archive at `file.path(): &Path` and close its handles.
//!     file.mark_as_processed()?;
//! }
//! # Ok(())
//! # }
//! ```

mod client;
mod fs;
mod stream;

pub use client::{EraClient, HttpClient};
pub use fs::{read_dir, read_era_dir, EraLocalMeta};
pub use stream::{EraMeta, EraRemoteMeta, EraStream, EraStreamConfig};

pub(crate) const BLOCKS_PER_FILE: usize = 8192;
