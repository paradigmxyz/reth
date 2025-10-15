//! Bincode compatibility support for reth primitive types.
//!
//! This module provides traits and implementations to work around bincode's limitations
//! with optional serde fields. The bincode crate requires all fields to be present during
//! serialization, which conflicts with types that have `#[serde(skip_serializing_if)]`
//! attributes for RPC compatibility.
//!
//! # Overview
//!
//! The main trait is `SerdeBincodeCompat`, which provides a conversion mechanism between
//! types and their bincode-compatible representations. There are two main ways to implement
//! this trait:
//!
//! 1. **Using RLP encoding** - Implement `RlpBincode` for types that already support RLP
//! 2. **Custom implementation** - Define a custom representation type
//!
//! # Examples
//!
//! ## Using with `serde_with`
//!
//! ```rust
//! # use reth_primitives_traits::serde_bincode_compat::{self, SerdeBincodeCompat};
//! # use serde::{Deserialize, Serialize};
//! # use serde_with::serde_as;
//! # use alloy_consensus::Header;
//! #[serde_as]
//! #[derive(Serialize, Deserialize)]
//! struct MyStruct {
//!     #[serde_as(as = "serde_bincode_compat::BincodeReprFor<'_, Header>")]
//!     data: Header,
//! }
//! ```

use alloc::vec::Vec;
use alloy_primitives::Bytes;
use core::fmt::Debug;
use serde::{de::DeserializeOwned, Serialize};

pub use super::{
    block::{serde_bincode_compat as block, serde_bincode_compat::*},
    header::{serde_bincode_compat as header, serde_bincode_compat::*},
};
pub use block_bincode::{Block, BlockBody};

/// Trait for types that can be serialized and deserialized using bincode.
///
/// This trait provides a workaround for bincode's incompatibility with optional
/// serde fields. It ensures all fields are serialized, making the type bincode-compatible.
///
/// # Implementation
///
/// The easiest way to implement this trait is using [`RlpBincode`] for RLP-encodable types:
///
/// ```rust
/// # use reth_primitives_traits::serde_bincode_compat::RlpBincode;
/// # use alloy_rlp::{RlpEncodable, RlpDecodable};
/// # #[derive(RlpEncodable, RlpDecodable)]
/// # struct MyType;
/// impl RlpBincode for MyType {}
/// // SerdeBincodeCompat is automatically implemented
/// ```
///
/// For custom implementations, see the examples in the `block` module.
///
/// The recommended way to add bincode compatible serialization is via the
/// [`serde_with`] crate and the `serde_as` macro. See for reference [`header`].
pub trait SerdeBincodeCompat: Sized + 'static {
    /// Serde representation of the type for bincode serialization.
    ///
    /// This type defines the bincode compatible serde format for the type.
    type BincodeRepr<'a>: Debug + Serialize + DeserializeOwned;

    /// Convert this type into its bincode representation
    fn as_repr(&self) -> Self::BincodeRepr<'_>;

    /// Convert from the bincode representation
    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self;
}

impl SerdeBincodeCompat for alloy_consensus::Header {
    type BincodeRepr<'a> = alloy_consensus::serde_bincode_compat::Header<'a>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        self.into()
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        repr.into()
    }
}

/// Type alias for the [`SerdeBincodeCompat::BincodeRepr`] associated type.
///
/// This provides a convenient way to refer to the bincode representation type
/// without having to write out the full associated type projection.
///
/// # Example
///
/// ```rust
/// # use reth_primitives_traits::serde_bincode_compat::{SerdeBincodeCompat, BincodeReprFor};
/// fn serialize_to_bincode<T: SerdeBincodeCompat>(value: &T) -> BincodeReprFor<'_, T> {
///     value.as_repr()
/// }
/// ```
pub type BincodeReprFor<'a, T> = <T as SerdeBincodeCompat>::BincodeRepr<'a>;

/// A helper trait for using RLP-encoding for providing bincode-compatible serialization.
///
/// By implementing this trait, [`SerdeBincodeCompat`] will be automatically implemented for the
/// type and RLP encoding will be used for serialization and deserialization for bincode
/// compatibility.
///
/// # Example
///
/// ```rust
/// # use reth_primitives_traits::serde_bincode_compat::RlpBincode;
/// # use alloy_rlp::{RlpEncodable, RlpDecodable};
/// #[derive(RlpEncodable, RlpDecodable)]
/// struct MyCustomType {
///     value: u64,
///     data: Vec<u8>,
/// }
///
/// // Simply implement the marker trait
/// impl RlpBincode for MyCustomType {}
///
/// // Now MyCustomType can be used with bincode through RLP encoding
/// ```
pub trait RlpBincode: alloy_rlp::Encodable + alloy_rlp::Decodable {}

impl<T: RlpBincode + 'static> SerdeBincodeCompat for T {
    type BincodeRepr<'a> = Bytes;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        let mut buf = Vec::new();
        self.encode(&mut buf);
        buf.into()
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        Self::decode(&mut repr.as_ref()).expect("Failed to decode bincode rlp representation")
    }
}

mod block_bincode {
    use crate::serde_bincode_compat::SerdeBincodeCompat;
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_consensus::TxEip4844;
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
            Self { header: SerdeBincodeCompat::from_repr(value.header), body: value.body.into() }
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

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
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
                transactions: value
                    .transactions
                    .into_iter()
                    .map(SerdeBincodeCompat::from_repr)
                    .collect(),
                ommers: value.ommers.into_iter().map(SerdeBincodeCompat::from_repr).collect(),
                withdrawals: value.withdrawals.into_owned(),
                block_access_list: None,
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

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }

    impl super::SerdeBincodeCompat for alloy_consensus::EthereumTxEnvelope<TxEip4844> {
        type BincodeRepr<'a> =
            alloy_consensus::serde_bincode_compat::transaction::EthereumTxEnvelope<'a>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }

    #[cfg(feature = "op")]
    impl super::SerdeBincodeCompat for op_alloy_consensus::OpTxEnvelope {
        type BincodeRepr<'a> =
            op_alloy_consensus::serde_bincode_compat::transaction::OpTxEnvelope<'a>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }
}
