use core::fmt::Debug;
use serde::{de::DeserializeOwned, Serialize};

pub use super::header::{serde_bincode_compat as header, serde_bincode_compat::*};
pub use block_bincode::BlockBody;

/// Trait for types that can be serialized and deserialized using bincode.
pub trait SerdeBincodeCompat: Sized + 'static {
    /// Serde representation of the type for bincode serialization.
    type BincodeRepr<'a>: Debug + Serialize + DeserializeOwned + From<&'a Self> + Into<Self>;
}

impl SerdeBincodeCompat for alloy_consensus::Header {
    type BincodeRepr<'a> = alloy_consensus::serde_bincode_compat::Header<'a>;
}

mod block_bincode {
    use crate::serde_bincode_compat::SerdeBincodeCompat;
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_consensus::serde_bincode_compat::Header;
    use alloy_eips::eip4895::Withdrawals;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`alloy_consensus::BlockBody`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives_traits::serde_bincode_compat::{self, SerdeBincodeCompat};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: SerdeBincodeCompat> {
    ///     #[serde_as(as = "serde_bincode_compat::BlockBody<'_, T>")]
    ///     body: alloy_consensus::BlockBody<T>,
    /// }
    /// ```
    #[derive(derive_more::Debug, Serialize, Deserialize)]
    #[debug(bound())]
    pub struct BlockBody<'a, T: SerdeBincodeCompat> {
        transactions: Vec<T::BincodeRepr<'a>>,
        ommers: Vec<Header<'a>>,
        withdrawals: Cow<'a, Option<Withdrawals>>,
    }

    impl<'a, T: SerdeBincodeCompat> From<&'a alloy_consensus::BlockBody<T>> for BlockBody<'a, T> {
        fn from(value: &'a alloy_consensus::BlockBody<T>) -> Self {
            Self {
                transactions: value.transactions.iter().map(Into::into).collect(),
                ommers: value.ommers.iter().map(Into::into).collect(),
                withdrawals: Cow::Borrowed(&value.withdrawals),
            }
        }
    }

    impl<'a, T: SerdeBincodeCompat> From<BlockBody<'a, T>> for alloy_consensus::BlockBody<T> {
        fn from(value: BlockBody<'a, T>) -> Self {
            Self {
                transactions: value.transactions.into_iter().map(Into::into).collect(),
                ommers: value.ommers.into_iter().map(Into::into).collect(),
                withdrawals: value.withdrawals.into_owned(),
            }
        }
    }

    impl<T: SerdeBincodeCompat> SerializeAs<alloy_consensus::BlockBody<T>> for BlockBody<'_, T> {
        fn serialize_as<S>(
            source: &alloy_consensus::BlockBody<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            BlockBody::from(source).serialize(serializer)
        }
    }

    impl<'de, T: SerdeBincodeCompat> DeserializeAs<'de, alloy_consensus::BlockBody<T>>
        for BlockBody<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<alloy_consensus::BlockBody<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            BlockBody::deserialize(deserializer).map(Into::into)
        }
    }

    impl<T: SerdeBincodeCompat> SerdeBincodeCompat for alloy_consensus::BlockBody<T> {
        type BincodeRepr<'a> = BlockBody<'a, T>;
    }
}
