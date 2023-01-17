//! Fixed hash types
use bytes::Buf;
use derive_more::{AsRef, Deref};
use fixed_hash::construct_fixed_hash;
use impl_serde::impl_fixed_hash_serde;
use reth_codecs::{impl_hash_compact, Compact};
use reth_rlp::{RlpDecodableWrapper, RlpEncodableWrapper, RlpMaxEncodedLen};

/// Implements a fixed hash type (eg. H512) with `serde`, `Arbitrary`, `proptest::Arbitrary` and
/// `Compact` support.
#[macro_export]
macro_rules! impl_fixed_hash_type {
    ($(($name:tt, $size:expr)),+) => {

        #[cfg(any(test, feature = "arbitrary"))]
        use proptest::{
            arbitrary::{any_with, ParamsFor},
            strategy::{BoxedStrategy, Strategy},
        };

        #[cfg(any(test, feature = "arbitrary"))]
        use arbitrary::Arbitrary;

        $(
            construct_fixed_hash! {
                #[cfg_attr(any(test, feature = "arbitrary"), derive(Arbitrary))]
                #[derive(AsRef, Deref, RlpEncodableWrapper, RlpDecodableWrapper, RlpMaxEncodedLen)]
                #[doc = concat!(stringify!($name), " fixed hash type.")]
                pub struct $name($size);
            }

            impl_hash_compact!($name);

            impl_fixed_hash_serde!($name, $size);

            #[cfg(any(test, feature = "arbitrary"))]
            impl proptest::arbitrary::Arbitrary for $name {
                type Parameters = ParamsFor<u8>;
                type Strategy = BoxedStrategy<$name>;

                fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
                    proptest::collection::vec(any_with::<u8>(args), $size)
                        .prop_map(move |vec| $name::from_slice(&vec))
                        .boxed()
                }
            }
        )+

        #[cfg(test)]
        mod hash_tests {
            use super::*;

            #[test]
            fn arbitrary() {
                $(
                    proptest::proptest!(|(field: $name)| {
                        let mut buf = vec![];
                        field.to_compact(&mut buf);

                        // Add noise. We want to make sure that only $size bytes get consumed.
                        buf.push(1);

                        let (decoded, remaining_buf) = $name::from_compact(&buf, buf.len());

                        assert!(field == decoded);
                        assert!(remaining_buf.len() == 1);
                    });
                )+
            }
        }
    };
}

impl_fixed_hash_type!((H64, 8), (H512, 64));
