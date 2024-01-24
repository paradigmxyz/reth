//! Implementation of consensus layer messages
pub mod message;
pub use message::*;
pub mod signature;
pub use signature::*;

use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs::derive_arbitrary;
use reth_primitives::Bytes;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Consensus layer message
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerConsensusMsg(pub Bytes);

#[cfg(test)]
mod tests {

    use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
    use reth_codecs::derive_arbitrary;
    use reth_primitives::hex;
    use reth_primitives::Bytes;
    use serde::{Deserialize, Serialize};

    #[derive_arbitrary(rlp)]
    #[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub(crate) struct TestClayerConsensusMessage {
        pub(crate) ctype: u8,
        pub(crate) body: Bytes,
    }

    #[derive_arbitrary(rlp)]
    #[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub(crate) struct TestViewChange {
        pub(crate) view: u64,
        pub(crate) sequence: u64,
    }

    #[test]
    fn message_encode() {
        let mut vc_out = vec![];
        let vc = TestViewChange { view: 1, sequence: 2 };
        vc.encode(&mut vc_out);

        let mut msg_out = vec![];
        let msg = TestClayerConsensusMessage { ctype: 10, body: vc_out.into() };
        msg.encode(&mut msg_out);

        println!("message: {:?}", hex::encode(msg_out.clone()));

        let msg2 = TestClayerConsensusMessage::decode(&mut msg_out.as_slice()).unwrap();
        println!("message2: {:?}", msg2);

        let vc2 = TestViewChange::decode(&mut msg2.body.to_vec().as_slice()).unwrap();
        println!("vc2: {:?}", vc2);
    }
}
