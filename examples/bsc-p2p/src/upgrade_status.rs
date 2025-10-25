//! Implement BSC upgrade message which is required during handshake with other BSC clients, e.g.,
//! geth.
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use bytes::{Buf, BufMut, Bytes, BytesMut};

/// The message id for the upgrade status message, used in the BSC handshake.
const UPGRADE_STATUS_MESSAGE_ID: u8 = 0x0b;

/// UpdateStatus packet introduced in BSC to notify peers whether to broadcast transaction or not.
/// It is used during the p2p handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpgradeStatus {
    /// Extension for support customized features for BSC.
    pub extension: UpgradeStatusExtension,
}

impl Encodable for UpgradeStatus {
    fn encode(&self, out: &mut dyn BufMut) {
        UPGRADE_STATUS_MESSAGE_ID.encode(out);
        self.extension.encode(out);
    }
}

impl Decodable for UpgradeStatus {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let message_id = u8::decode(buf)?;
        if message_id != UPGRADE_STATUS_MESSAGE_ID {
            return Err(alloy_rlp::Error::Custom("Invalid message ID"));
        }
        buf.advance(1);
        let extension = UpgradeStatusExtension::decode(buf)?;
        Ok(Self { extension })
    }
}

impl UpgradeStatus {
    /// Creates a new `UpgradeStatus` with the specified extension.
    #[allow(dead_code)]
    pub fn new(extension: UpgradeStatusExtension) -> Self {
        Self { extension }
    }

    /// Creates a new `UpgradeStatus` that allows peer transaction broadcasting.
    pub fn allow_broadcast() -> Self {
        Self { extension: UpgradeStatusExtension::allow_broadcast() }
    }

    /// Creates a new `UpgradeStatus` that disables peer transaction broadcasting.
    #[allow(dead_code)]
    pub fn disable_broadcast() -> Self {
        Self { extension: UpgradeStatusExtension::disable_broadcast() }
    }

    /// Encode the upgrade status message into RLPx bytes.
    pub fn into_rlpx(self) -> Bytes {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        out.freeze()
    }
}

/// The extension to define whether to enable or disable the flag.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpgradeStatusExtension {
    /// To notify a peer to disable the broadcast of transactions or not.
    pub disable_peer_tx_broadcast: bool,
}

impl UpgradeStatusExtension {
    /// Creates a new `UpgradeStatusExtension` with the specified broadcast setting.
    #[allow(dead_code)]
    pub fn new(disable_peer_tx_broadcast: bool) -> Self {
        Self { disable_peer_tx_broadcast }
    }

    /// Creates a new `UpgradeStatusExtension` that allows peer transaction broadcasting.
    pub fn allow_broadcast() -> Self {
        Self { disable_peer_tx_broadcast: false }
    }

    /// Creates a new `UpgradeStatusExtension` that disables peer transaction broadcasting.
    #[allow(dead_code)]
    pub fn disable_broadcast() -> Self {
        Self { disable_peer_tx_broadcast: true }
    }
}
