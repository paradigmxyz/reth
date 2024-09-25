// todo: expand this (examples, assumptions, invariants)
//! Execution extensions (`ExEx`).
//!
//! An execution extension is a task that derives its state from Reth's state.
//!
//! Some examples of such state derives are rollups, bridges, and indexers.
//!
//! An `ExEx` is a [`Future`] resolving to a `Result<()>` that is run indefinitely alongside Reth.
//!
//! `ExEx`'s are initialized using an async closure that resolves to the `ExEx`; this closure gets
//! passed an [`ExExContext`] where it is possible to spawn additional tasks and modify Reth.
//!
//! Most `ExEx`'s will want to derive their state from the [`CanonStateNotification`] channel given
//! in [`ExExContext`]. A new notification is emitted whenever blocks are executed in live and
//! historical sync.
//!
//! # Pruning
//!
//! `ExEx`'s **SHOULD** emit an `ExExEvent::FinishedHeight` event to signify what blocks have been
//! processed. This event is used by Reth to determine what state can be pruned.
//!
//! An `ExEx` will only receive notifications for blocks greater than the block emitted in the
//! event. To clarify: if the `ExEx` emits `ExExEvent::FinishedHeight(0)` it will receive
//! notifications for any `block_number > 0`.
//!
//! [`Future`]: std::future::Future
//! [`ExExContext`]: crate::ExExContext
//! [`CanonStateNotification`]: reth_provider::CanonStateNotification
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod backfill;
pub use backfill::*;

mod context;
pub use context::*;

mod event;
pub use event::*;

mod manager;
pub use manager::*;

mod notifications;
pub use notifications::*;

mod wal;
pub use wal::*;

// Re-export exex types
#[doc(inline)]
pub use reth_exex_types::*;
