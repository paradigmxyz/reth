#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![allow(clippy::result_large_err)]
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
//! # use reth_db::mdbx::test_utils::create_test_rw_db;
//! # use reth_db::mdbx::{Env, WriteMap};
//! # use reth_downloaders::bodies::bodies::BodiesDownloaderBuilder;
//! # use reth_downloaders::headers::reverse_headers::ReverseHeadersDownloaderBuilder;
//! # use reth_interfaces::consensus::Consensus;
//! # use reth_interfaces::sync::NoopSyncStateUpdate;
//! # use reth_interfaces::test_utils::{TestBodiesClient, TestConsensus, TestHeadersClient, TestStatusUpdater};
//! # use reth_primitives::PeerId;
//! # use reth_stages::Pipeline;
//! # use reth_stages::sets::DefaultStages;
//! # let consensus: Arc<dyn Consensus> = Arc::new(TestConsensus::default());
//! # let headers_downloader = ReverseHeadersDownloaderBuilder::default().build(
//! #    Arc::new(TestHeadersClient::default()),
//! #    consensus.clone()
//! # );
//! # let bodies_downloader = BodiesDownloaderBuilder::default().build(
//! #    Arc::new(TestBodiesClient { responder: |_| Ok((PeerId::zero(), vec![]).into()) }),
//! #    consensus.clone(),
//! #    create_test_rw_db()
//! # );
//! # let (status_updater, _) = TestStatusUpdater::new();
//! // Create a pipeline that can fully sync
//! # let pipeline: Pipeline<Env<WriteMap>, NoopSyncStateUpdate> =
//! Pipeline::builder()
//!     .add_stages(
//!         DefaultStages::new(consensus, headers_downloader, bodies_downloader, status_updater)
//!     )
//!     .build();
//! ```
mod error;
mod id;
mod pipeline;
mod stage;
mod trie;
mod util;

/// The real database type we use in Reth using MDBX.
pub type DefaultDB<'a> = LatestStateProviderRef<'a, 'a, Tx<'a, RW, WriteMap>>;
use reth_db::mdbx::{tx::Tx, WriteMap, RW};
use reth_provider::LatestStateProviderRef;

#[allow(missing_docs)]
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

/// A re-export of common structs and traits.
pub mod prelude;

/// Implementations of stages.
pub mod stages;

pub mod sets;

pub use error::*;
pub use id::*;
pub use pipeline::*;
pub use stage::*;
pub use trie::DBTrieLoader;

// NOTE: Needed so the link in the module-level rustdoc works.
#[allow(unused_extern_crates)]
extern crate metrics as metrics_core;
