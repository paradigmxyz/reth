use crate::{
    errors::{EthHandshakeError, EthStreamError, P2PStreamError},
    ethstream::MAX_STATUS_SIZE,
    CanDisconnect,
};
use bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream};
use reth_eth_wire_types::{
    DisconnectReason, EthMessage, EthNetworkPrimitives, ProtocolMessage, Status, StatusMessage,
};
use reth_ethereum_forks::ForkFilter;
use reth_primitives_traits::GotExpected;
use std::{fmt::Debug, future::Future, pin::Pin, time::Duration};
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tracing::{debug, trace};

/// A trait that knows how to perform the P2P handshake.
pub trait EthRlpxHandshake: Debug + Send + Sync + 'static {
    /// Perform the P2P handshake for the `eth` protocol.
    fn handshake<'a>(
        &'a self,
        unauth: &'a mut dyn UnauthEth,
        status: Status,
        fork_filter: ForkFilter,
        timeout_limit: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Status, EthStreamError>> + 'a + Send>>;
}

/// An unauthenticated stream that can send and receive messages.
pub trait UnauthEth:
    Stream<Item = Result<BytesMut, P2PStreamError>>
    + Sink<Bytes, Error = P2PStreamError>
    + CanDisconnect<Bytes>
    + Unpin
    + Send
{
}

impl<T> UnauthEth for T where
    T: Stream<Item = Result<BytesMut, P2PStreamError>>
        + Sink<Bytes, Error = P2PStreamError>
        + CanDisconnect<Bytes>
        + Unpin
        + Send
{
}

/// The Ethereum P2P handshake.
///
/// This performs the regular ethereum `eth` rlpx handshake.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct EthHandshake;

impl EthRlpxHandshake for EthHandshake {
    fn handshake<'a>(
        &'a self,
        unauth: &'a mut dyn UnauthEth,
        status: Status,
        fork_filter: ForkFilter,
        timeout_limit: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Status, EthStreamError>> + 'a + Send>> {
        Box::pin(async move {
            timeout(timeout_limit, EthereumEthHandshake(unauth).eth_handshake(status, fork_filter))
                .await
                .map_err(|_| EthStreamError::StreamTimeout)?
        })
    }
}

/// A type that performs the ethereum specific `eth` protocol handshake.
#[derive(Debug)]
pub struct EthereumEthHandshake<'a, S: ?Sized>(pub &'a mut S);

impl<S: ?Sized, E> EthereumEthHandshake<'_, S>
where
    S: Stream<Item = Result<BytesMut, E>> + CanDisconnect<Bytes> + Send + Unpin,
    EthStreamError: From<E> + From<<S as Sink<Bytes>>::Error>,
{
    /// Performs the `eth` rlpx protocol handshake using the given input stream.
    pub async fn eth_handshake(
        self,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Result<Status, EthStreamError> {
        let unauth = self.0;
        // Send our status message
        let status_msg =
            alloy_rlp::encode(ProtocolMessage::<EthNetworkPrimitives>::from(EthMessage::<
                EthNetworkPrimitives,
            >::Status(
                StatusMessage::Legacy(status),
            )))
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
            unauth
                .disconnect(DisconnectReason::ProtocolBreach)
                .await
                .map_err(EthStreamError::from)?;
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
            EthMessage::Status(their_status_message) => {
                trace!("Validating incoming ETH status from peer");

                if status.genesis != their_status_message.genesis() {
                    unauth
                        .disconnect(DisconnectReason::ProtocolBreach)
                        .await
                        .map_err(EthStreamError::from)?;
                    return Err(EthHandshakeError::MismatchedGenesis(
                        GotExpected {
                            expected: status.genesis,
                            got: their_status_message.genesis(),
                        }
                        .into(),
                    )
                    .into());
                }

                if status.version != their_status_message.version() {
                    unauth
                        .disconnect(DisconnectReason::ProtocolBreach)
                        .await
                        .map_err(EthStreamError::from)?;
                    return Err(EthHandshakeError::MismatchedProtocolVersion(GotExpected {
                        got: their_status_message.version(),
                        expected: status.version,
                    })
                    .into());
                }

                if status.chain != *their_status_message.chain() {
                    unauth
                        .disconnect(DisconnectReason::ProtocolBreach)
                        .await
                        .map_err(EthStreamError::from)?;
                    return Err(EthHandshakeError::MismatchedChain(GotExpected {
                        got: *their_status_message.chain(),
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
                if let Err(err) = fork_filter
                    .validate(their_status_message.forkid())
                    .map_err(EthHandshakeError::InvalidFork)
                {
                    unauth
                        .disconnect(DisconnectReason::ProtocolBreach)
                        .await
                        .map_err(EthStreamError::from)?;
                    return Err(err.into());
                }

                Ok(their_status_message.to_legacy())
            }
            _ => {
                unauth
                    .disconnect(DisconnectReason::ProtocolBreach)
                    .await
                    .map_err(EthStreamError::from)?;
                Err(EthStreamError::EthHandshakeError(
                    EthHandshakeError::NonStatusMessageInHandshake,
                ))
            }
        }
    }
}
