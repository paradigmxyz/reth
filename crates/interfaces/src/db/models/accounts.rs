//! Account related models and types.

use reth_codecs::main_codec;
use reth_primitives::{Account, Address};

/// Account as it is saved inside [`AccountChangeSet`]. [`Address`] is the subkey.
#[main_codec]
#[derive(Debug, Default)]
pub struct AccountBeforeTx {
    address: Address,
    info: Account,
}
