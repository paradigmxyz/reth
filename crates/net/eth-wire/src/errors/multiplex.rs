use std::{error, fmt};

use reth_primitives::BytesMut;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::Capability;

use super::P2PStreamError;

#[derive(Error, Default, Debug)]
#[error("some error was thrown in multiplexing, result primary: {primary:?}, errors subprotocols: {subprotocols:?}, errors multiplex: {multiplex:?}")]
pub struct MultiplexResult<PrimaryOk, PrimaryErr>
where
    PrimaryOk: fmt::Debug,
    PrimaryErr: error::Error,
{
    primary: Option<Result<PrimaryOk, PrimaryErr>>,
    subprotocols: Vec<(Capability, SubprotocolError)>,
    multiplex: Vec<MultiplexError>,
}

impl<PrimaryOk, PrimaryErr> MultiplexResult<PrimaryOk, PrimaryErr>
where
    PrimaryOk: fmt::Debug,
    PrimaryErr: error::Error,
{
    pub fn new() -> Self {
        MultiplexResult { primary: None, subprotocols: vec![], multiplex: vec![] }
    }

    pub fn is_none(&self) -> bool {
        self.primary.is_none() && self.subprotocols.is_empty() && self.multiplex.is_empty()
    }

    pub fn set_primary_ok(mut self, ok: PrimaryOk) -> Self {
        self.primary = Some(Ok(ok));
        self
    }

    pub fn set_primary_error(mut self, e: PrimaryErr) -> Self {
        self.primary = Some(Err(e));
        self
    }

    pub fn add_subprotocol_error(&mut self, cap: Capability, e: SubprotocolError) {
        self.subprotocols.push((cap, e))
    }

    pub fn add_multiplex_error(&mut self, e: impl Into<MultiplexError>) {
        self.multiplex.push(e.into())
    }
}

#[derive(Error, Debug)]
pub enum SubprotocolError {
    #[error("cap handle closed")]
    HandleClosed,
}

#[derive(Error, Debug)]
pub enum MultiplexError {
    /// Error of the underlying P2P connection.
    #[error(transparent)]
    P2PStream(#[from] P2PStreamError),
    #[error("failed to delegate bytes from p2p, {0}")]
    DelegateFailed(#[from] mpsc::error::SendError<BytesMut>),
}
