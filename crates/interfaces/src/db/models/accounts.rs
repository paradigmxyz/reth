//! Account related models and types.

use crate::db::{
    table::{Decode, Encode},
    Error,
};
use bytes::Bytes;
use eyre::eyre;
use reth_codecs::main_codec;
use reth_primitives::{Account, Address, TxNumber};

/// Account as it is saved inside [`AccountChangeSet`]. [`Address`] is the subkey.
#[main_codec]
#[derive(Debug, Default, Clone)]
pub struct AccountBeforeTx {
    /// Address for the account. Acts as `DupSort::SubKey`.
    address: Address,
    /// Address for the account. Acts as `DupSort::SubKey`.
    info: Account,
}

/// [`TxNumber`] concatenated with [`Address`]. Used as a key for [`StorageChangeSet`]
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TxNumberAddress(pub (TxNumber, Address));

impl TxNumberAddress {
    /// Consumes `Self` and returns [`TxNumber`], [`Address`]
    pub fn take(self) -> (TxNumber, Address) {
        (self.0 .0, self.0 .1)
    }
}

impl From<(u64, Address)> for TxNumberAddress {
    fn from(tpl: (u64, Address)) -> Self {
        TxNumberAddress(tpl)
    }
}

impl Encode for TxNumberAddress {
    type Encoded = [u8; 28];

    fn encode(self) -> Self::Encoded {
        let tx = self.0 .0;
        let address = self.0 .1;

        let mut buf = [0u8; 28];

        buf[..8].copy_from_slice(&tx.to_be_bytes());
        buf[8..].copy_from_slice(address.as_bytes());
        buf
    }
}

impl Decode for TxNumberAddress {
    fn decode<B: Into<Bytes>>(value: B) -> Result<Self, Error> {
        let value: bytes::Bytes = value.into();

        let num = u64::from_be_bytes(
            value.as_ref()[..8]
                .try_into()
                .map_err(|_| Error::Decode(eyre!("Into bytes error.")))?,
        );
        let hash = Address::decode(value.slice(8..))?;

        Ok(TxNumberAddress((num, hash)))
    }
}
