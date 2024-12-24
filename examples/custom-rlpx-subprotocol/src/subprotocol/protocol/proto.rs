//! Simple RLPx Ping Pong protocol that also support sending messages,
//! following [RLPx specs](https://github.com/ethereum/devp2p/blob/master/rlpx.md)

use alloy_primitives::bytes::{Buf, BufMut, BytesMut};
use reth_eth_wire::{protocol::Protocol, Capability};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustomRlpxProtoMessageId {
    Ping = 0x00,
    Pong = 0x01,
    PingMessage = 0x02,
    PongMessage = 0x03,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CustomRlpxProtoMessageKind {
    Ping,
    Pong,
    PingMessage(String),
    PongMessage(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CustomRlpxProtoMessage {
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
        Protocol::new(Self::capability(), 4)
    }

    /// Creates a ping message
    pub fn ping_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: CustomRlpxProtoMessageId::PingMessage,
            message: CustomRlpxProtoMessageKind::PingMessage(msg.into()),
        }
    }
    /// Creates a ping message
    pub fn pong_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: CustomRlpxProtoMessageId::PongMessage,
            message: CustomRlpxProtoMessageKind::PongMessage(msg.into()),
        }
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

    /// Creates a new `CustomRlpxProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            CustomRlpxProtoMessageKind::Ping | CustomRlpxProtoMessageKind::Pong => {}
            CustomRlpxProtoMessageKind::PingMessage(msg) |
            CustomRlpxProtoMessageKind::PongMessage(msg) => {
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
            0x02 => CustomRlpxProtoMessageId::PingMessage,
            0x03 => CustomRlpxProtoMessageId::PongMessage,
            _ => return None,
        };
        let message = match message_type {
            CustomRlpxProtoMessageId::Ping => CustomRlpxProtoMessageKind::Ping,
            CustomRlpxProtoMessageId::Pong => CustomRlpxProtoMessageKind::Pong,
            CustomRlpxProtoMessageId::PingMessage => CustomRlpxProtoMessageKind::PingMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
            CustomRlpxProtoMessageId::PongMessage => CustomRlpxProtoMessageKind::PongMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
        };

        Some(Self { message_type, message })
    }
}
