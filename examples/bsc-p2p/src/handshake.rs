use derive_more::Debug;
use futures::SinkExt;
use reth_eth_wire::{
    errors::{EthHandshakeError, EthStreamError},
    handshake::{handshake, Handshake, UnauthEth},
};
use reth_eth_wire_types::{
    upgrade_status::{UpgradeStatus, UpgradeStatusExtension},
    DisconnectReason, EthMessage, EthNetworkPrimitives, EthVersion, ProtocolMessage, Status,
};
use reth_ethereum_forks::ForkFilter;
use std::{future::Future, pin::Pin};
use tokio::time::{timeout, Duration};
use tokio_stream::StreamExt;
use tracing::debug;

#[derive(Debug)]
/// The Binance Smart Chain (BSC) P2P handshake.
pub struct BscHandshake;

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

impl Handshake for BscHandshake {
    fn handshake<'a>(
        &'a self,
        unauth: &'a mut dyn UnauthEth,
        status: Status,
        fork_filter: ForkFilter,
        timeout_limit: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Status, EthStreamError>> + 'a + Send>> {
        Box::pin(async move {
            let fut = async {
                let negotiated_status = handshake(unauth, status, fork_filter).await?;
                Self::upgrade_status(unauth, negotiated_status).await
            };
            timeout(timeout_limit, fut).await.map_err(|_| EthStreamError::StreamTimeout)?
        })
    }
}
