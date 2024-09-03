//! This crate includes the core components for advancing a reth chain.
//!
//! ## Functionality
//!
//! The components in this crate are involved in:
//! * Handling and reacting to incoming consensus events ([`EngineHandler`](engine::EngineHandler))
//! * Advancing the chain ([`ChainOrchestrator`](chain::ChainOrchestrator))
//! * Keeping track of the chain structure in-memory ([`TreeState`](tree::TreeState))
//! * Performing backfill sync and handling its progress ([`BackfillSync`](backfill::BackfillSync))
//! * Downloading blocks ([`BlockDownloader`](download::BlockDownloader)), and
//! * Persisting blocks and performing pruning
//!   ([`PersistenceService`](persistence::PersistenceService))
//!
//! ## Design and motivation
//!
//! The node must keep up with the state of the chain and validate new updates to the chain state.
//!
//! In order to respond to consensus messages and advance the chain quickly, validation code must
//! avoid database write operations and perform as much work as possible in-memory. This requirement
//! is what informs the architecture of the components this crate.
//!
//! ## Chain synchronization
//!
//! When the node receives a block with an unknown parent, or cannot find a certain block hash, it
//! needs to download and validate the part of the chain that is missing.
//!
//! This can happen during a live sync when the node receives a forkchoice update from the consensus
//! layer which causes the node to have to walk back from the received head, downloading the block's
//! parents until it reaches a known block.
//!
//! This can also technically happen when a finalized block is fetched, before checking distance,
//! but this is a very unlikely case.
//!
//! There are two mutually-exclusive ways the node can bring itself in sync with the chain:
//! * Backfill sync: downloading and validating large ranges of blocks in a structured manner,
//!   performing different parts of the validation process in sequence.
//! * Live sync: By responding to new blocks from a connected consensus layer and downloading any
//!   missing blocks on the fly.
//!
//! To determine which sync type to use, the node checks how many blocks it needs to execute to
//! catch up to the tip of the chain. If this range is large, backfill sync will be used. Otherwise,
//! live sync will be used.
//!
//! The backfill sync is driven by components which implement the
//! [`BackfillSync`](backfill::BackfillSync) trait, like [`PipelineSync`](backfill::PipelineSync).
//!
//! ## Handling consensus messages
//!
//! Consensus message handling is performed by three main components:
//! 1. The [`EngineHandler`](engine::EngineHandler), which takes incoming consensus mesesages and
//!    manages any requested backfill or download work.
//! 2. The [`EngineApiRequestHandler`](engine::EngineApiRequestHandler), which processes messages
//!    from the [`EngineHandler`](engine::EngineHandler) and delegates them to the
//!    [`EngineApiTreeHandler`](tree::EngineApiTreeHandler).
//! 3. The [`EngineApiTreeHandler`](tree::EngineApiTreeHandler), which processes incoming tree
//!    events, such as new payload events, sending back requests for any needed backfill or download
//!    work.
//!
//! ## Chain representation
//!
//! The chain is represented by the [`TreeState`](tree::TreeState) data structure, which keeps
//! tracks of blocks by hash and number, as well as keeping track of parent-child relationships
//! between blocks. The hash and number of the current head of the canonical chain is also tracked
//! in the [`TreeState`](tree::TreeState).
//!
//! ## Persistence model
//!
//! Because the node minimizes database writes in the critical path for handling consensus messages,
//! it must perform database writes in the background.
//!
//! Performing writes in the background has two advantages:
//! 1. As mentioned, writes are not in the critical path of request processing.
//! 2. Writes can be larger and done at a lower frequency, allowing for more efficient writes.
//!
//! This is achieved by spawning a separate thread which is sent different commands corresponding to
//! different types of writes, for example a command to write a list of transactions, or remove a
//! specific range of blocks.
//!
//! The persistence service must also respond to these commands, to ensure that any in-memory state
//! that is on-disk can be cleaned up, conserving memory and allowing us to add new blocks
//! indefinitely.
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
