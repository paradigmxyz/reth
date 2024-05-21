use reth_eth_wire::{protocol::Protocol, Capability};
use reth_network::protocol::{ConnectionHandler, ProtocolHandler};
use reth_primitives::{Buf, BufMut, BytesMut};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomRlpxProtoMessageId {
    Ping = 0x00,
    Pong = 0x01,
    CustomMessage = 0x02,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustomRlpxProtoMessageKind {
    Ping,
    Pong,
    CustomMessage(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomRlpxProtoMessage {
    pub message_type: CustomRlpxProtoMessageId,
    pub message: CustomRlpxProtoMessageKind,
}

impl CustomRlpxProtoMessage {
    /// Returns the capability for the `custom_rlpx` protocol.
    pub fn capability() -> Capability {
        Capability::new_static("custom_rlpx", 1)
    }

    /// Returns the protocol for the `custom_rlpx` protocol.
    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 3)
    }

    /// Creates a ping message
    pub fn ping() -> Self {
        Self {
            message_type: CustomRlpxProtoMessageId::Ping,
            message: CustomRlpxProtoMessageKind::Ping,
        }
    }

    /// Creates a pong message
    pub fn pong() -> Self {
        Self {
            message_type: CustomRlpxProtoMessageId::Pong,
            message: CustomRlpxProtoMessageKind::Pong,
        }
    }

    /// Creates a custom message
    pub fn custom_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: CustomRlpxProtoMessageId::CustomMessage,
            message: CustomRlpxProtoMessageKind::CustomMessage(msg.into()),
        }
    }

    /// Creates a new `CustomRlpxProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            CustomRlpxProtoMessageKind::Ping => {}
            CustomRlpxProtoMessageKind::Pong => {}
            CustomRlpxProtoMessageKind::CustomMessage(msg) => {
                buf.put(msg.as_bytes());
            }
        }
        buf
    }

    /// Decodes a `CustomRlpxProtoMessage` from the given message buffer.
    pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        let id = buf[0];
        buf.advance(1);
        let message_type = match id {
            0x00 => CustomRlpxProtoMessageId::Ping,
            0x01 => CustomRlpxProtoMessageId::Pong,
            0x02 => CustomRlpxProtoMessageId::CustomMessage,
            _ => return None,
        };
        let message = match message_type {
            CustomRlpxProtoMessageId::Ping => CustomRlpxProtoMessageKind::Ping,
            CustomRlpxProtoMessageId::Pong => CustomRlpxProtoMessageKind::Pong,
            CustomRlpxProtoMessageId::CustomMessage => CustomRlpxProtoMessageKind::CustomMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
        };
        Some(Self { message_type, message })
    }
}
