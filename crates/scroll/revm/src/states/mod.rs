//! Scroll `revm` states types redefinitions.

pub use account_info::ScrollAccountInfo;
mod account_info;

pub use bundle::{ScrollBundleBuilder, ScrollBundleState};
mod bundle;

pub use bundle_account::ScrollBundleAccount;
mod bundle_account;

pub use changes::{ScrollPlainStateReverts, ScrollStateChangeset};
mod changes;

pub use reverts::{ScrollAccountInfoRevert, ScrollAccountRevert, ScrollReverts};
mod reverts;
