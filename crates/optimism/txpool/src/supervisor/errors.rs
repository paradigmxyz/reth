use core::error;

/// Failures occurring during validation of inbox entries.
#[derive(thiserror::Error, Debug)]
pub enum InteropTxValidatorError {
    /// RPC client failure.
    #[error("supervisor rpc client failure: {0}")]
    Client(Box<dyn error::Error + Send + Sync>),

    /// Inbox entry validation against the Supervisor took longer than allowed.
    #[error("inbox entry validation timed out, timeout: {0} secs")]
    Timeout(u64),

    /// Catch-all variant for other supervisor server errors.
    #[error("supervisor server error: {0}")]
    Server(Box<dyn error::Error + Send + Sync>),
}

impl InteropTxValidatorError {
    /// Returns a new instance of [`Client`](Self::Client) error variant.
    pub fn client<E>(err: alloy_json_rpc::RpcError<E>) -> Self
    where
        E: error::Error + Send + Sync + 'static,
    {
        Self::Client(Box::new(err))
    }

    /// Returns a new instance of [`Server`](Self::Server) error variant.
    pub fn server(err: impl error::Error + Send + Sync + 'static) -> Self {
        Self::Server(Box::new(err))
    }
}
