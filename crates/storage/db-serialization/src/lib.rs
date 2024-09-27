//! DB serialization.
//!
//! This is for db serialization . It is used to serialize and deserialize data

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod encode;
pub use encode::Encode;
mod decode;
pub use decode::Decode;

mod error;
pub use error::DecodeError;

/// Macro that implements [`Encode`] and [`Decode`] for uint types.
macro_rules! impl_uints {
    ($($name:tt),+) => {
        $(
            impl Encode for $name {
                type Encoded = [u8; core::mem::size_of::<$name>()];

                fn encode(self) -> Self::Encoded {
                    self.to_be_bytes()
                }
            }

            impl Decode for $name {
                fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, $crate::DecodeError> {
                    Ok(
                        $name::from_be_bytes(
                            value.as_ref().try_into().map_err(|_| $crate::DecodeError)?
                        )
                    )
                }
            }
        )+
    };
}

impl_uints!(u64, u32, u16, u8);
