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

    fn db_name(&self) -> &str; //string::String<Bytes>; todo
}

pub trait DupSort: Table {
    type SubKey: Object;
}
