//! This crate includes the core components for advancing a reth chain.
//!
//! ## Functionality
//!
//! The components in this crate are involved in:
//! * Handling and reacting to incoming consensus events ([`EngineHandler`])
//! * Advancing the chain ([`ChainOrchestrator`])
//! * Keeping track of the chain structure in-memory ([`TreeState`])
//! * Performing backfill sync and handling its progress ([`BackfillSync`])
//! * Downloading blocks ([`BlockDownloader`]), and
//! * Persisting blocks and performing pruning ([`PersistenceService`])
//!
//! ## Design and motivation
//!
//! The job of a node is to keep up to date with the state of the chain and validate new updates to
//! the chain state. Performing this task quickly is often the bottleneck for high performance
//! blockchain systems.
//!
//! TODO:
//! * How to design the critical path to be as fast as possible -> do as much as possible in memory,
//!   minimize db writes
//!
//! ## Chain synchronization
//!
//! When the node receives a block with an unknown parent, or cannot find a certain block hash, it
//! needs to download and validate the part of the chain that is missing.
//!
//! There are two mutually-exclusive ways the node can bring itself in sync with the chain:
//! * Backfill sync: downloading and validating large ranges of blocks in a structured manner,
//!   performing different parts of the validation process in sequence.
//! * Live sync: By responding to new blocks from a connected consensus layer and downloading any
//!   missing blocks on the fly.
//!
//! To determine which sync type to use, the node checks how many blocks it needs to execute to
//! catch up to the tip of the chain. If this range is large, backfill sync will be used. Otherwise,
//! life sync will be used.
//!
//! The backfill sync is driven by components which implement the [`BackfillSync`] trait, like
//! [`PipelineSync`].
//!
//! TODO:
//! * not sure what else to put here
//!
//! ## Handling consensus messages
//!
//! When a consensus message is received, the node first checks if the message requires any action
//! from the node, for example downloading a missing parent block or syncing a long range of chain
//! history. These checks, and
//!
//! TODO:
//! The three layers
//! * [`EngineHandler`] -> take incoming events, poll request handler, manage download / pipeline loop
//! * [`EngineRequestHandler`] -> take incoming requests, poll tree, send any download / pipeline requests back
//! * [`EngineApiTreeHandlerImpl`] -> take incoming tree events, send validaiton + download / pipeline
//!   requests back
//!
//! Finally, a new payload is stored in-memory before the node returns a response to the consensus
//! client.
//!
//! ## Chain representation
//!
//! The chain is represented by the [`TreeState`] data structure, which consists of two maps which
//! track each block by hash and by number, respectively.
//! The hash and number of the current tip of the canonical chain is also tracked in the
//! [`TreeState`].
//!
//! TODO:
//! * explain `EngineApiTreeHandlerImpl`?
//!
//! ## Persistence model
//!
//! Because the node minimizes database writes in the critical path for handling consensus messages,
//! it must perform them in at some other point, in the background.
//!
//! Performing writes in the background has two advantages:
//! 1. As mentioned, writes are not in the critical path of request processing.
//! 2. Writes can be larger and done at a lower frequency, allowing for more efficient writes.
//!
//! This is achieved by spawning a separate thread which is sent different commands corresponding to
//! different types of writes, for example a command to write a list of transaction, or remove a
//! specific range of blocks.
//!
//! Additionally, the persistence service must respond to these commands, to ensure that any
//! in-memory state that is on-disk can be cleaned up, conserving memory and allowing us to add new
//! blocks indefinitely.
//!
//! TODO:
//! * not sure what else to put here
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

/// Re-export of the blockchain tree API.
pub use reth_blockchain_tree_api::*;

/// Support for backfill sync mode.
pub mod backfill;
/// The type that drives the chain forward.
pub mod chain;
/// Support for downloading blocks on demand for live sync.
pub mod download;
/// Engine Api chain handler support.
pub mod engine;
/// Metrics support.
pub mod metrics;
/// The background writer service, coordinating write operations on static files and the database.
pub mod persistence;
/// Support for interacting with the blockchain tree.
pub mod tree;

/// Test utilities.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
