//! Block execution changes with the reverts needed to persist and unwind them.
//!
//! Ported from the `revm-database` crate of revm (<https://github.com/bluealloy/revm>),
//! Copyright (c) 2021-2026 draganrakita, licensed under the MIT License (see `LICENSES/MIT.txt`).
//! The code is kept close to upstream, so upstream style is allowed below.

#![allow(
    clippy::collapsible_if,
    clippy::doc_markdown,
    clippy::explicit_into_iter_loop,
    clippy::explicit_iter_loop,
    clippy::if_not_else,
    clippy::match_same_arms,
    clippy::module_inception,
    clippy::redundant_clone,
    clippy::redundant_else,
    clippy::use_self
)]

mod account_status;
pub use account_status::AccountStatus;

mod bundle_account;
pub use bundle_account::BundleAccount;

mod bundle_state;
pub use bundle_state::{BundleBuilder, BundleRetention, BundleState, OriginalValuesKnown};

mod changes;
pub use changes::{
    PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset, StorageRevert,
};

mod plain_account;
pub use plain_account::{PlainAccount, PlainStorage, StorageSlot, StorageWithOriginalValues};

mod reverts;
pub use reverts::{AccountInfoRevert, AccountRevert, RevertToSlot, Reverts};

mod transition_account;
pub use transition_account::TransitionAccount;

mod transition_state;
pub use transition_state::TransitionState;
