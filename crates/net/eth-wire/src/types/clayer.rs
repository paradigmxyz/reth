use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs::derive_arbitrary;
use reth_primitives::Bytes;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Consensus layer message

#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerConsensusMsg(pub Vec<Bytes>);
