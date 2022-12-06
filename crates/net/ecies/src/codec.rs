use crate::{algorithm::ECIES, ECIESError, EgressECIESValue, IngressECIESValue};
use bytes::BytesMut;
use reth_primitives::H512 as PeerId;
use secp256k1::SecretKey;
use std::{fmt::Debug, io};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{instrument, trace};

/// Tokio codec for ECIES
#[derive(Debug)]
pub(crate) struct ECIESCodec {
    ecies: ECIES,
    state: ECIESState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Current ECIES state of a connection
enum ECIESState {
    /// The first stage of the ECIES handshake, where each side of the connection sends an auth
    /// message containing the ephemeral public key, signature of the public key, nonce, and other
    /// metadata.
    Auth,

    /// The second stage of the ECIES handshake, where each side of the connection sends an ack
    /// message containing the nonce and other metadata.
    Ack,
    Header,
    Body,
}

impl ECIESCodec {
    /// Create a new server codec using the given secret key
    pub(crate) fn new_server(secret_key: SecretKey) -> Result<Self, ECIESError> {
        Ok(Self { ecies: ECIES::new_server(secret_key)?, state: ECIESState::Auth })
    }

    /// Create a new client codec using the given secret key and the server's public id
    pub(crate) fn new_client(secret_key: SecretKey, remote_id: PeerId) -> Result<Self, ECIESError> {
        Ok(Self { ecies: ECIES::new_client(secret_key, remote_id)?, state: ECIESState::Auth })
    }
}

impl Decoder for ECIESCodec {
    type Item = IngressECIESValue;
    type Error = ECIESError;

    #[instrument(level = "trace", skip_all, fields(peer=&*format!("{:?}", self.ecies.remote_id.map(|s| s.to_string())), state=&*format!("{:?}", self.state)))]
    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.state {
                ECIESState::Auth => {
                    trace!("parsing auth");
                    if buf.len() < 2 {
                        return Ok(None)
                    }

                    let payload_size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    let total_size = payload_size + 2;

                    if buf.len() < total_size {
                        trace!("current len {}, need {}", buf.len(), total_size);
                        return Ok(None)
                    }

                    self.ecies.read_auth(&mut buf.split_to(total_size))?;

                    self.state = ECIESState::Header;
                    return Ok(Some(IngressECIESValue::AuthReceive(self.ecies.remote_id())))
                }
                ECIESState::Ack => {
                    trace!("parsing ack with len {}", buf.len());
                    if buf.len() < 2 {
                        return Ok(None)
                    }

                    let payload_size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    let total_size = payload_size + 2;

                    if buf.len() < total_size {
                        trace!("current len {}, need {}", buf.len(), total_size);
                        return Ok(None)
                    }

                    self.ecies.read_ack(&mut buf.split_to(total_size))?;

                    self.state = ECIESState::Header;
                    return Ok(Some(IngressECIESValue::Ack))
                }
                ECIESState::Header => {
                    if buf.len() < ECIES::header_len() {
                        trace!("current len {}, need {}", buf.len(), ECIES::header_len());
                        return Ok(None)
                    }

                    self.ecies.read_header(&mut buf.split_to(ECIES::header_len()))?;

                    self.state = ECIESState::Body;
                }
                ECIESState::Body => {
                    if buf.len() < self.ecies.body_len() {
                        return Ok(None)
                    }

                    let mut data = buf.split_to(self.ecies.body_len());
                    let mut ret = BytesMut::new();
                    ret.extend_from_slice(self.ecies.read_body(&mut data)?);

                    self.state = ECIESState::Header;
                    return Ok(Some(IngressECIESValue::Message(ret)))
                }
            }
        }
    }
}

impl Encoder<EgressECIESValue> for ECIESCodec {
    type Error = io::Error;

    #[instrument(level = "trace", skip(self, buf), fields(peer=&*format!("{:?}", self.ecies.remote_id.map(|s| s.to_string())), state=&*format!("{:?}", self.state)))]
    fn encode(&mut self, item: EgressECIESValue, buf: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            EgressECIESValue::Auth => {
                self.state = ECIESState::Ack;
                self.ecies.write_auth(buf);
                Ok(())
            }
            EgressECIESValue::Ack => {
                self.state = ECIESState::Header;
                self.ecies.write_ack(buf);
                Ok(())
            }
            EgressECIESValue::Message(data) => {
                self.ecies.write_header(buf, data.len());
                self.ecies.write_body(buf, &data);
                Ok(())
            }
        }
    }
}
