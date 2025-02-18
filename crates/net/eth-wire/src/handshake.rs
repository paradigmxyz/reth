use crate::{
    errors::{EthHandshakeError, EthStreamError, P2PStreamError},
    ethstream::MAX_STATUS_SIZE,
    CanDisconnect,
};
use bytes::{Bytes, BytesMut};
use derive_more::Debug;
use futures::{Sink, SinkExt, Stream};
use reth_eth_wire_types::{
    upgrade_status::{UpgradeStatus, UpgradeStatusExtension},
    DisconnectReason, EthMessage, EthNetworkPrimitives, EthVersion, ProtocolMessage, Status,
};
use reth_ethereum_forks::ForkFilter;
use reth_primitives_traits::GotExpected;
use std::{future::Future, pin::Pin};
use tokio_stream::StreamExt;
use tracing::{debug, trace};

/// A trait that knows how to perform the P2P handshake.
pub trait Handshake: Debug + Send + Sync + 'static {
    /// Perform the P2P handshake.
    fn handshake<'a>(
        &'a self,
        unauth: &'a mut dyn UnauthEth,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Pin<Box<dyn Future<Output = Result<Status, EthStreamError>> + 'a + Send>>;
}

/// An unauthenticated [`EthStream`].
pub trait UnauthEth:
    Stream<Item = Result<BytesMut, P2PStreamError>>
    + Sink<Bytes, Error = P2PStreamError>
    + CanDisconnect<Bytes>
    + Unpin
    + Send
{
}

#[derive(Debug)]
/// The Binance Smart Chain (BSC) P2P handshake.
pub struct BscHandshake;

impl Handshake for BscHandshake {
    fn handshake<'a>(
        &'a self,
        unauth: &'a mut dyn UnauthEth,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Pin<Box<dyn Future<Output = Result<Status, EthStreamError>> + 'a + Send>> {
        Box::pin(async move {
            let negotiated_status = handshake(unauth, status, fork_filter).await?;
            Self::upgrade_status(unauth, negotiated_status).await?;
            Ok(negotiated_status)
        })
    }
}

#[derive(Debug)]
/// The Ethereum P2P handshake.
pub struct EthHandshake;

impl Handshake for EthHandshake {
    fn handshake<'a>(
        &'a self,
        unauth: &'a mut dyn UnauthEth,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Pin<Box<dyn Future<Output = Result<Status, EthStreamError>> + 'a + Send>> {
        Box::pin(async move { handshake(unauth, status, fork_filter).await })
    }
}

impl BscHandshake {
    /// Negotiate the upgrade status message.
    pub async fn upgrade_status(
        unauth: &mut dyn UnauthEth,
        negotiated_status: Status,
    ) -> Result<Status, EthStreamError> {
        if negotiated_status.version > EthVersion::Eth66 {
            // Send upgrade status message
            let upgrade_msg =
                alloy_rlp::encode(ProtocolMessage::<EthNetworkPrimitives>::from(EthMessage::<
                    EthNetworkPrimitives,
                >::UpgradeStatus(
                    UpgradeStatus {
                        extension: UpgradeStatusExtension { disable_peer_tx_broadcast: false },
                    },
                )))
                .into();
            unauth.send(upgrade_msg).await?;

            // Receive peer's upgrade status response
            let their_msg = match unauth.next().await {
                Some(Ok(msg)) => msg,
                Some(Err(e)) => return Err(EthStreamError::from(e)),
                None => {
                    unauth.disconnect(DisconnectReason::DisconnectRequested).await?;
                    return Err(EthStreamError::EthHandshakeError(EthHandshakeError::NoResponse));
                }
            };

            // Decode their response
            let msg = ProtocolMessage::<EthNetworkPrimitives>::decode_message(
                negotiated_status.version,
                &mut their_msg.as_ref(),
            )
            .map_err(|err| {
                debug!("Decode error in BSC handshake: msg={their_msg:x}");
                EthStreamError::InvalidMessage(err)
            })?;

            // Validate their response
            if let EthMessage::UpgradeStatus(_) = msg.message {
                return Ok(negotiated_status);
            }

            // if they sent a non-upgrade status message, disconnect
            unauth.disconnect(DisconnectReason::ProtocolBreach).await?;
            return Err(EthStreamError::EthHandshakeError(
                EthHandshakeError::NonStatusMessageInHandshake,
            ));
        }

        Ok(negotiated_status)
    }
}

/// A **shared helper function** that performs the common handshake logic.
async fn handshake(
    unauth: &mut dyn UnauthEth,
    status: Status,
    fork_filter: ForkFilter,
) -> Result<Status, EthStreamError> {
    // Send our status message
    let status_msg =
        alloy_rlp::encode(ProtocolMessage::<EthNetworkPrimitives>::from(EthMessage::<
            EthNetworkPrimitives,
        >::Status(status)))
        .into();
    unauth.send(status_msg).await.map_err(EthStreamError::from)?;

    // Receive peer's response
    let their_msg_res = unauth.next().await;
    let their_msg = match their_msg_res {
        Some(Ok(msg)) => msg,
        Some(Err(e)) => return Err(EthStreamError::from(e)),
        None => {
            unauth
                .disconnect(DisconnectReason::DisconnectRequested)
                .await
                .map_err(EthStreamError::from)?;
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::NoResponse));
        }
    };

    if their_msg.len() > MAX_STATUS_SIZE {
        unauth.disconnect(DisconnectReason::ProtocolBreach).await.map_err(EthStreamError::from)?;
        return Err(EthStreamError::MessageTooBig(their_msg.len()));
    }

    let version = status.version;
    let msg = match ProtocolMessage::<EthNetworkPrimitives>::decode_message(
        version,
        &mut their_msg.as_ref(),
    ) {
        Ok(m) => m,
        Err(err) => {
            debug!("decode error in eth handshake: msg={their_msg:x}");
            unauth
                .disconnect(DisconnectReason::DisconnectRequested)
                .await
                .map_err(EthStreamError::from)?;
            return Err(EthStreamError::InvalidMessage(err));
        }
    };

    // Validate peer response
    match msg.message {
        EthMessage::Status(their_status) => {
            trace!("Validating incoming ETH status from peer");

            if status.genesis != their_status.genesis {
                unauth
                    .disconnect(DisconnectReason::ProtocolBreach)
                    .await
                    .map_err(EthStreamError::from)?;
                return Err(EthHandshakeError::MismatchedGenesis(
                    GotExpected { expected: status.genesis, got: their_status.genesis }.into(),
                )
                .into());
            }

            if status.version != their_status.version {
                unauth
                    .disconnect(DisconnectReason::ProtocolBreach)
                    .await
                    .map_err(EthStreamError::from)?;
                return Err(EthHandshakeError::MismatchedProtocolVersion(GotExpected {
                    got: their_status.version,
                    expected: status.version,
                })
                .into());
            }

            if status.chain != their_status.chain {
                unauth
                    .disconnect(DisconnectReason::ProtocolBreach)
                    .await
                    .map_err(EthStreamError::from)?;
                return Err(EthHandshakeError::MismatchedChain(GotExpected {
                    got: their_status.chain,
                    expected: status.chain,
                })
                .into());
            }

            // Ensure total difficulty is reasonable
            if status.total_difficulty.bit_len() > 160 {
                unauth
                    .disconnect(DisconnectReason::ProtocolBreach)
                    .await
                    .map_err(EthStreamError::from)?;
                return Err(EthHandshakeError::TotalDifficultyBitLenTooLarge {
                    got: status.total_difficulty.bit_len(),
                    maximum: 160,
                }
                .into());
            }

            // Fork validation
            if let Err(err) =
                fork_filter.validate(their_status.forkid).map_err(EthHandshakeError::InvalidFork)
            {
                unauth
                    .disconnect(DisconnectReason::ProtocolBreach)
                    .await
                    .map_err(EthStreamError::from)?;
                return Err(err.into());
            }

            Ok(their_status)
        }
        _ => {
            unauth
                .disconnect(DisconnectReason::ProtocolBreach)
                .await
                .map_err(EthStreamError::from)?;
            Err(EthStreamError::EthHandshakeError(EthHandshakeError::NonStatusMessageInHandshake))
        }
    }
}
