//! An asynchronous stream interface for downloading ERA1 files.
//!
//! # Examples
//! ```
//! use futures_util::StreamExt;
//! use reqwest::{Client, Url};
//! use reth_era_downloader::{EraClient, EraStream};
//! use std::{path::PathBuf, str::FromStr};
//!
//! # async fn f() -> Result<(), Box<dyn std::error::Error + 'static>> {
//! // URL where the ERA1 files are hosted
//! let url = Url::from_str("file:///")?;
//!
//! // Directory where the ERA1 files will be downloaded to
//! let folder = PathBuf::new().into_boxed_path();
//!
//! let client = EraClient::new(Client::new(), url, folder);
//!
//! // Keep up to 2 ERA1 files in the `folder`.
//! // More downloads won't start until some of the files are removed.
//! let max_files = 2;
//!
//! // Do not download more than 2 files at the same time.
//! let max_concurrent_downloads = 2;
//!
//! let mut stream = EraStream::new(client, max_files, max_concurrent_downloads);
//!
//! # return Ok(());
//! while let Some(file) = stream.next().await {
//!     let file = file?;
//!     // Process `file: Box<Path>`
//! }
//! # Ok(())
//! # }
//! ```

mod client;
mod stream;

pub use client::EraClient;
pub use stream::EraStream;
