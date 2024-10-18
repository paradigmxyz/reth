//! EIP-7685 requests.

use alloc::vec::Vec;
use derive_more::{Deref, DerefMut, From, IntoIterator};
use revm_primitives::Bytes;
use serde::{Deserialize, Serialize};

/// A list of opaque EIP-7685 requests.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    Hash,
    Deref,
    DerefMut,
    From,
    IntoIterator,
    Serialize,
    Deserialize,
)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct Requests(Vec<Bytes>);

impl Requests {
    /// Consumes [`Requests`] and returns the inner raw opaque requests.
    pub fn take(self) -> Vec<Bytes> {
        self.0
    }
}
