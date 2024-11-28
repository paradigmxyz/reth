use crate::{hash_code, POSEIDON_EMPTY};
use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

/// The extension for a Scroll account's representation in storage.
///
/// The extension is used in order to maintain backwards compatibility if more fields need to be
/// added to the account (see [reth codecs](reth_codecs::test_utils) for details).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "reth-codec", derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct AccountExtension {
    /// Size in bytes of the account's bytecode.
    pub code_size: u64,
    /// Poseidon hash of the account's bytecode. `None` means there is no
    /// bytecode for the account.
    pub poseidon_code_hash: Option<B256>,
}

impl AccountExtension {
    /// Creates an empty [`AccountExtension`].
    pub const fn empty() -> Self {
        Self { code_size: 0, poseidon_code_hash: None }
    }

    /// Creates an [`AccountExtension`] from the provided bytecode.
    pub fn from_bytecode<T: AsRef<[u8]>>(code: &T) -> Self {
        let code = code.as_ref();
        Self {
            code_size: code.len() as u64,
            poseidon_code_hash: (!code.is_empty()).then_some(hash_code(code)),
        }
    }
}

impl From<(u64, B256)> for AccountExtension {
    fn from(value: (u64, B256)) -> Self {
        Self {
            code_size: value.0,
            poseidon_code_hash: (value.1 != POSEIDON_EMPTY).then_some(value.1),
        }
    }
}
