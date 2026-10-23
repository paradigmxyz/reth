//! Block execution changes with the reverts needed to persist and unwind them.
//!
//! Ported from the `revm-database` crate of revm (<https://github.com/bluealloy/revm>),
//! Copyright (c) 2021-2026 draganrakita, licensed under the MIT License (see `LICENSES/MIT.txt`).

mod account;
pub use account::{BundleAccount, StorageSlot, StorageWithOriginalValues};

mod changes;
pub use changes::{PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset};

mod reverts;
pub use reverts::{AccountInfoRevert, AccountRevert, RevertToSlot, Reverts};

mod state;
pub use state::{BundleBuilder, BundleState, OriginalValuesKnown};

mod status;
pub use status::AccountStatus;

mod transition;
pub use transition::TransitionAccount;
