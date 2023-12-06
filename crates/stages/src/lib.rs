//! Staged syncing primitives for reth.
//!
//! This crate contains the syncing primitives [`Pipeline`] and [`Stage`], as well as all stages
//! that reth uses to sync.
//!
//! A pipeline can be configured using [`Pipeline::builder()`].
//!
//! For ease of use, this crate also exposes a set of [`StageSet`]s, which are collections of stages
//! that perform specific functions during sync. Stage sets can be customized; it is possible to
//! add, disable and replace stages in the set.
//!
//! # Examples
//!
//! ```
//! # use std::sync::Arc;
//! # use reth_downloaders::bodies::bodies::BodiesDownloaderBuilder;
//! # use reth_downloaders::headers::reverse_headers::ReverseHeadersDownloaderBuilder;
//! # use reth_interfaces::consensus::Consensus;
//! # use reth_interfaces::test_utils::{TestBodiesClient, TestConsensus, TestHeadersClient};
//! # use reth_revm::EvmProcessorFactory;
//! # use reth_primitives::{PeerId, MAINNET, B256};
//! # use reth_stages::Pipeline;
//! # use reth_stages::sets::DefaultStages;
//! # use tokio::sync::watch;
//! # use reth_provider::ProviderFactory;
//! # use reth_provider::HeaderSyncMode;
//! # use reth_provider::test_utils::create_test_provider_factory;
//! #
//! # let chain_spec = MAINNET.clone();
//! # let consensus: Arc<dyn Consensus> = Arc::new(TestConsensus::default());
//! # let headers_downloader = ReverseHeadersDownloaderBuilder::default().build(
//! #    Arc::new(TestHeadersClient::default()),
//! #    consensus.clone()
//! # );
//! # let provider_factory = create_test_provider_factory();
//! # let bodies_downloader = BodiesDownloaderBuilder::default().build(
//! #    Arc::new(TestBodiesClient { responder: |_| Ok((PeerId::ZERO, vec![]).into()) }),
//! #    consensus.clone(),
//! #    provider_factory.clone()
//! # );
//! # let (tip_tx, tip_rx) = watch::channel(B256::default());
//! # let executor_factory = EvmProcessorFactory::new(chain_spec.clone());
//! // Create a pipeline that can fully sync
//! # let pipeline =
//! Pipeline::builder()
//!     .with_tip_sender(tip_tx)
//!     .add_stages(DefaultStages::new(
//!         provider_factory.clone(),
//!         HeaderSyncMode::Tip(tip_rx),
//!         consensus,
//!         headers_downloader,
//!         bodies_downloader,
//!         executor_factory,
//!     ))
//!     .build(provider_factory);
//! ```
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![allow(clippy::result_large_err)] // TODO(danipopes): fix this
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod error;
mod metrics;
mod pipeline;
mod stage;
mod util;

#[allow(missing_docs)]
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

/// A re-export of common structs and traits.
pub mod prelude;

/// Implementations of stages.
pub mod stages;

pub mod sets;

pub use crate::metrics::*;
pub use error::*;
pub use pipeline::*;
pub use stage::*;
