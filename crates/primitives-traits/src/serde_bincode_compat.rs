use core::fmt::Debug;
use serde::{de::DeserializeOwned, Serialize};

pub use super::{
    block::{serde_bincode_compat as block, serde_bincode_compat::*},
    header::{serde_bincode_compat as header, serde_bincode_compat::*},
};
pub use block_bincode::{Block, BlockBody};

/// Trait for types that can be serialized and deserialized using bincode.
///
/// The recommended way to add bincode compatible serialization is via the
/// [`serde_with`] crate and the `serde_as` macro that. See for reference [`header`].
pub trait SerdeBincodeCompat: Sized + 'static {
    /// Serde representation of the type for bincode serialization.
    ///
    /// This type defines the bincode compatible serde format for the type.
    type BincodeRepr<'a>: Debug + Serialize + DeserializeOwned + Into<Self>;

    /// Convert this type into its bincode representation
    fn as_repr(&self) -> Self::BincodeRepr<'_>;
}

impl SerdeBincodeCompat for alloy_consensus::Header {
    type BincodeRepr<'a> = alloy_consensus::serde_bincode_compat::Header<'a>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        self.into()
    }
}

/// Type alias for the [`SerdeBincodeCompat::BincodeRepr`] associated type.
pub type BincodeReprFor<'a, T> = <T as SerdeBincodeCompat>::BincodeRepr<'a>;

mod block_bincode {
    use crate::serde_bincode_compat::SerdeBincodeCompat;
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_eips::eip4895::Withdrawals;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`alloy_consensus::Block`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use alloy_consensus::Block;
    /// use reth_primitives_traits::serde_bincode_compat::{self, SerdeBincodeCompat};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: SerdeBincodeCompat, H: SerdeBincodeCompat> {
    ///     #[serde_as(as = "serde_bincode_compat::Block<'_, T, H>")]
    ///     body: Block<T, H>,
    /// }
    /// ```
    #[derive(derive_more::Debug, Serialize, Deserialize)]
    #[debug(bound())]
    pub struct Block<'a, T: SerdeBincodeCompat, H: SerdeBincodeCompat> {
        header: H::BincodeRepr<'a>,
        #[serde(bound = "BlockBody<'a, T, H>: Serialize + serde::de::DeserializeOwned")]
        body: BlockBody<'a, T, H>,
    }

    impl<'a, T: SerdeBincodeCompat, H: SerdeBincodeCompat> From<&'a alloy_consensus::Block<T, H>>
        for Block<'a, T, H>
    {
        fn from(value: &'a alloy_consensus::Block<T, H>) -> Self {
            Self { header: value.header.as_repr(), body: (&value.body).into() }
        }
    }

    impl<'a, T: SerdeBincodeCompat, H: SerdeBincodeCompat> From<Block<'a, T, H>>
        for alloy_consensus::Block<T, H>
    {
        fn from(value: Block<'a, T, H>) -> Self {
            Self { header: value.header.into(), body: value.body.into() }
        }
    }

    impl<T: SerdeBincodeCompat, H: SerdeBincodeCompat> SerializeAs<alloy_consensus::Block<T, H>>
        for Block<'_, T, H>
    {
        fn serialize_as<S>(
            source: &alloy_consensus::Block<T, H>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Block::from(source).serialize(serializer)
        }
    }

    impl<'de, T: SerdeBincodeCompat, H: SerdeBincodeCompat>
        DeserializeAs<'de, alloy_consensus::Block<T, H>> for Block<'de, T, H>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<alloy_consensus::Block<T, H>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Block::deserialize(deserializer).map(Into::into)
        }
    }

    impl<T: SerdeBincodeCompat, H: SerdeBincodeCompat> SerdeBincodeCompat
        for alloy_consensus::Block<T, H>
    {
        type BincodeRepr<'a> = Block<'a, T, H>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }
    }

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
    /// struct Data<T: SerdeBincodeCompat, H: SerdeBincodeCompat> {
    ///     #[serde_as(as = "serde_bincode_compat::BlockBody<'_, T, H>")]
    ///     body: alloy_consensus::BlockBody<T, H>,
    /// }
    /// ```
    #[derive(derive_more::Debug, Serialize, Deserialize)]
    #[debug(bound())]
    pub struct BlockBody<'a, T: SerdeBincodeCompat, H: SerdeBincodeCompat> {
        transactions: Vec<T::BincodeRepr<'a>>,
        ommers: Vec<H::BincodeRepr<'a>>,
        withdrawals: Cow<'a, Option<Withdrawals>>,
    }

    impl<'a, T: SerdeBincodeCompat, H: SerdeBincodeCompat>
        From<&'a alloy_consensus::BlockBody<T, H>> for BlockBody<'a, T, H>
    {
        fn from(value: &'a alloy_consensus::BlockBody<T, H>) -> Self {
            Self {
                transactions: value.transactions.iter().map(|tx| tx.as_repr()).collect(),
                ommers: value.ommers.iter().map(|h| h.as_repr()).collect(),
                withdrawals: Cow::Borrowed(&value.withdrawals),
            }
        }
    }

    impl<'a, T: SerdeBincodeCompat, H: SerdeBincodeCompat> From<BlockBody<'a, T, H>>
        for alloy_consensus::BlockBody<T, H>
    {
        fn from(value: BlockBody<'a, T, H>) -> Self {
            Self {
                transactions: value.transactions.into_iter().map(Into::into).collect(),
                ommers: value.ommers.into_iter().map(Into::into).collect(),
                withdrawals: value.withdrawals.into_owned(),
            }
        }
    }

    impl<T: SerdeBincodeCompat, H: SerdeBincodeCompat> SerializeAs<alloy_consensus::BlockBody<T, H>>
        for BlockBody<'_, T, H>
    {
        fn serialize_as<S>(
            source: &alloy_consensus::BlockBody<T, H>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            BlockBody::from(source).serialize(serializer)
        }
    }

    impl<'de, T: SerdeBincodeCompat, H: SerdeBincodeCompat>
        DeserializeAs<'de, alloy_consensus::BlockBody<T, H>> for BlockBody<'de, T, H>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<alloy_consensus::BlockBody<T, H>, D::Error>
        where
            D: Deserializer<'de>,
        {
            BlockBody::deserialize(deserializer).map(Into::into)
        }
    }

    impl<T: SerdeBincodeCompat, H: SerdeBincodeCompat> SerdeBincodeCompat
        for alloy_consensus::BlockBody<T, H>
    {
        type BincodeRepr<'a> = BlockBody<'a, T, H>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }
    }
}
