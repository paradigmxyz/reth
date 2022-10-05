use bytes::Bytes;
use reth_primitives::Address;
use std::fmt::Debug;

pub trait Encode: Send + Sync + Sized {
    type Encoded: AsRef<[u8]> + Send + Sync;

    fn encode(self) -> Self::Encoded;
}

pub trait Decode: Send + Sync + Sized {
    fn decode(b: &[u8]) -> eyre::Result<Self>;
}

pub trait Object: Encode + Decode {}

impl<T> Object for T where T: Encode + Decode {}

pub trait Table: Send + Sync + Debug + 'static {
    type Key: Encode;
    type Value: Object;
    type SeekKey: Encode;

    fn db_name(&self) -> &'static str; //string::String<Bytes>; TODO
}

pub trait DupSort: Table {
    type SubKey: Object;
}

impl Encode for Vec<u8> {
    type Encoded = Self;

    fn encode(self) -> Self::Encoded {
        self
    }
}

impl Decode for Vec<u8> {
    fn decode(b: &[u8]) -> eyre::Result<Self> {
        Ok(b.to_vec())
    }
}

impl Encode for Bytes {
    type Encoded = Self;

    fn encode(self) -> Self::Encoded {
        self
    }
}

impl Decode for Bytes {
    fn decode(b: &[u8]) -> eyre::Result<Self> {
        Ok(b.to_vec().into())
    }
}

impl Encode for Address {
    type Encoded = [u8; 20];

    fn encode(self) -> Self::Encoded {
        self.0
    }
}

impl Decode for Address {
    fn decode(b: &[u8]) -> eyre::Result<Self> {
        Ok(Address::from_slice(b))
    }
}
